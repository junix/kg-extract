//! Agentic single-session extractor (experimental).
//!
//! Unlike [`SimpleExtractor`](super::SimpleExtractor) — which runs each chunk in
//! its *own* SDK session concurrently and merges afterwards — this drives the
//! **whole document through one continuous multi-turn conversation**:
//!
//! 1. The full text is written to `document.md` in an isolated temp directory,
//!    and the SDK's `claude` runs with its working directory set there, in a
//!    **read-only tool sandbox** (Read / Grep / Glob allowed; Write / Edit / Bash
//!    denied). So the agent sees only this document and can `grep`/`Read` it for
//!    context it needs, but can't touch anything else.
//! 2. The text is split into slices; each slice is fed as one **turn** in the
//!    same session, so slice *N*'s extraction has the memory of slices 1..N-1
//!    (cross-slice coreference for free).
//! 3. After every slice, a final **whole-graph relation-gleaning** turn names the
//!    entities that ended up with no edge and asks the model to connect them —
//!    now able to link across slices, not just within one.
//!
//! Trade-off vs `SimpleExtractor`: this is strictly sequential (no chunk
//! concurrency) and the conversation context grows with the document, so it
//! suits coherence-critical, small-to-medium docs rather than very large ones.
//!
//! SDK-only: it needs the `claude-agent-sdk-rs` client for the filesystem cwd,
//! tool sandbox, and a single long-lived session, so it talks to the SDK
//! directly rather than through the generic [`LlmBackend`](crate::backend::LlmBackend).
//!
//! Module layout:
//! - [`prompts`] — the system / slice / gleaning / schema prompt templates.
//! - [`schema_filter`] — closed-world schema validation ([`SchemaPolicy`]).
//! - this file — the [`AgenticExtractor`] struct, its [`Extractor`] impl, and
//!   the single-session extraction driver.

mod prompts;
mod schema_filter;

use schema_filter::{norm_type, SchemaPolicy};

/// The accumulators one finished agentic session produced, handed to
/// [`AgenticExtractor::assemble_response`] to build the final
/// [`ExtractionResponse`]. Held as a struct so the assembly helper stays under
/// clippy's argument limit and is easy to construct in tests.
struct SessionOutcome {
    slices_count: usize,
    total_tool_uses: usize,
    entities: HashMap<String, Entity>,
    triples: Vec<Triple>,
    parsed_results: Vec<ParsedResult>,
    /// Fixed mode: how many records were dropped across all slices.
    total_dropped: usize,
    /// Fixed mode: the out-of-schema type names seen.
    dropped_types: BTreeSet<String>,
    /// Evolving mode: node types proposed outside the seed.
    new_nodes: BTreeSet<String>,
    /// Evolving mode: relation types proposed outside the seed.
    new_relations: BTreeSet<String>,
}
use prompts::{
    RELATION_GLEANING_PROMPT, SCHEMA_EVOLVING, SCHEMA_FEEDBACK, SCHEMA_HINTS, SCHEMA_REDO_PROMPT,
    SCHEMA_STRICT, SLICE_PROMPT, SYSTEM_PROMPT,
};

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use claude_agent_sdk_rs::types::SystemPrompt;
use claude_agent_sdk_rs::{
    ClaudeAgentOptions, ClaudeSdkClient, ContentBlock, Message as SdkMessage, PermissionMode,
};
use futures::StreamExt;

use super::{validate_input, Extractor};
use crate::backend::sdk_agent::provider_env;
use crate::chunking::{segment, Segment};
use crate::extractor::simple::{entity_type_tokens, parse_output, parse_relations_against};
use crate::types::{
    Entity, ExtractionConfig, ExtractionResponse, KnowledgeGraph, ParsedResult, Triple,
};

#[cfg(test)]
use crate::types::{Schema, SchemaMode};

/// Merge one slice's entity into the running entity table. First occurrence
/// wins (the prompt asks the model to only add what is NEW each turn); under
/// a repeat occurrence still contributes its provenance, so an entity mentioned
/// in several slices cites all of them.
fn merge_slice_entity(all: &mut HashMap<String, Entity>, id: &str, e: &Entity) {
    match all.entry(id.to_string()) {
        std::collections::hash_map::Entry::Occupied(mut _o) => {
            crate::citation::union_citations(&mut _o.get_mut().metadata, &e.metadata);
        }
        std::collections::hash_map::Entry::Vacant(v) => {
            v.insert(e.clone());
        }
    }
}

/// Format the per-slice "proposed N new type(s)" log line for the Evolving
/// schema arm. Returns `None` when no new types were proposed (the caller's
/// quiet-gate), otherwise the ready-to-`eprintln!` string. Pure so the
/// BTreeSet-join formatting can be unit-tested.
fn new_types_proposal_log(
    slice_index: usize,
    new_nodes: &BTreeSet<String>,
    new_relations: &BTreeSet<String>,
) -> Option<String> {
    if new_nodes.is_empty() && new_relations.is_empty() {
        return None;
    }
    let csv = new_nodes
        .iter()
        .chain(new_relations.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "  [schema] slice {} proposed {} new type(s): {csv}",
        slice_index + 1,
        new_nodes.len() + new_relations.len()
    ))
}

/// Stamp a slice's line-range provenance onto every record it produced: each
/// entity, and each triple plus its two endpoint snapshots. No-op when the
/// slice carries no line metadata (`lines == None`). Endpoint snapshots matter
/// because `add_triple` re-inserts endpoints into the entity table, where an
/// unstamped copy would erase provenance (the union only keeps what's present).
fn stamp_slice_citations(parsed: &mut ParsedResult, cite: &crate::citation::Citation) {
    for e in parsed.entities.values_mut() {
        crate::citation::attach_citation(&mut e.metadata, cite);
    }
    for t in parsed.triples.iter_mut() {
        crate::citation::attach_citation(&mut t.metadata, cite);
        crate::citation::attach_citation(&mut t.subject.metadata, cite);
        crate::citation::attach_citation(&mut t.object.metadata, cite);
    }
}

/// Commit one slice's parse into the running accumulators without any schema
/// filtering — the shared tail of the `SchemaPolicy::Off` and `Evolving` arms
/// (both keep everything; Evolving additionally records new types first).
/// Each entity is merged with first-occurrence-wins citation unioning, the
/// triples are appended, and the parse is kept for the response.
fn commit_unconstrained_slice(
    all_entities: &mut HashMap<String, Entity>,
    all_triples: &mut Vec<Triple>,
    parsed_results: &mut Vec<ParsedResult>,
    parsed: ParsedResult,
) {
    for (id, e) in &parsed.entities {
        merge_slice_entity(all_entities, id, e);
    }
    all_triples.extend(parsed.triples.clone());
    parsed_results.push(parsed);
}

/// Experimental single-session, sandboxed, multi-turn extractor.
pub struct AgenticExtractor {
    config: ExtractionConfig,
    /// Agent provider name (`minimaxcc` / `glmcc` / `mimocc`).
    agent: String,
    /// Whole-graph relation-gleaning rounds after all slices (0 = off).
    max_relation_gleanings: usize,
    pub quiet: bool,
}

impl AgenticExtractor {
    /// Default config mirrors [`SimpleExtractor`](super::SimpleExtractor):
    /// `segment_size` 5000, `min_segment_size` 100. The model is pinned by the
    /// provider env, so `model_name` is only a label here.
    pub fn default_config() -> ExtractionConfig {
        ExtractionConfig {
            model_name: "agent".into(),
            segment_size: 5000,
            min_segment_size: 100,
            ..Default::default()
        }
    }

    pub fn new(agent: &str) -> Self {
        AgenticExtractor {
            config: Self::default_config(),
            agent: agent.to_string(),
            max_relation_gleanings: 0,
            quiet: false,
        }
    }

    pub fn with_config(agent: &str, config: ExtractionConfig) -> Self {
        AgenticExtractor {
            config,
            agent: agent.to_string(),
            max_relation_gleanings: 0,
            quiet: false,
        }
    }

    /// Enable whole-graph relation-gleaning with `n` rounds (builder style).
    pub fn relation_gleanings(mut self, n: usize) -> Self {
        self.max_relation_gleanings = n;
        self
    }

    /// Set the schema mode (builder style). With a non-empty schema:
    /// [`crate::types::SchemaMode::Fixed`] validates each slice and drops
    /// out-of-schema records (reminding the model on the next turn);
    /// [`crate::types::SchemaMode::Evolving`] keeps everything but records the
    /// types used outside the seed as `new_schema_types`.
    /// [`crate::types::SchemaMode::Open`] (and either constrained mode with an
    /// empty schema) leaves extraction unconstrained.
    pub fn schema_mode(mut self, mode: crate::types::SchemaMode) -> Self {
        self.config.spec.mode = mode;
        self
    }

    /// Slice the text the same way the Simple engine does: a single slice when
    /// the text fits in `segment_size`, otherwise segmented (dropping a tiny
    /// trailing fragment). Each slice keeps its char offsets so provenance can
    /// be line-mapped.
    fn slices(&self, text: &str) -> Vec<Segment> {
        if text.chars().count() > self.config.segment_size {
            segment(
                text,
                self.config.chunker,
                self.config.segment_size,
                self.config.overlap,
            )
            .into_iter()
            .filter(|s| !(s.content.chars().count() < self.config.min_segment_size && s.index > 0))
            .collect()
        } else {
            vec![Segment {
                content: text.to_string(),
                index: 0,
                start: 0,
                end: text.chars().count(),
                range: Some(core_types_rs::SourceRange {
                    char_span: core_types_rs::CharSpan::new(0, text.chars().count()),
                    ..core_types_rs::SourceRange::default()
                }),
            }]
        }
    }

    fn rel_types(&self) -> String {
        if self.config.predicates_list().is_empty() {
            "related_to, part_of, uses, produces, includes, has_property".to_string()
        } else {
            self.config.predicates_list().join(", ")
        }
    }

    /// Build one slice turn's base prompt. `{i}` is 1-based (the human-facing
    /// "slice N of M"); `{n}` is the total slice count.
    fn slice_prompt(i: usize, n: usize, slice: &str) -> String {
        SLICE_PROMPT
            .replace("{i}", &(i + 1).to_string())
            .replace("{n}", &n.to_string())
            .replace("{slice}", slice)
    }

    /// The stern re-do prompt, fired when a whole slice came back entirely
    /// out-of-schema. Carries the full allowed type vocabulary so the model
    /// can map each thing onto the closest fit.
    fn redo_prompt(entity_types: &str, rel_types: &str) -> String {
        SCHEMA_REDO_PROMPT
            .replace("{entity_types}", entity_types)
            .replace("{relationship_types}", rel_types)
    }

    /// Format the per-turn feedback that re-anchors a drifting model after a
    /// slice dropped out-of-schema records. The `{dropped_types}` slot is filled
    /// with ` (dropped: CSV)` when any types were dropped, or the empty string
    /// otherwise (so the sentence still reads naturally). Returns `None` when
    /// there is nothing to feed back (`dropped_records == 0`).
    fn drop_feedback(
        dropped_records: usize,
        dropped_types: &BTreeSet<String>,
        entity_types: &str,
        rel_types: &str,
    ) -> Option<String> {
        if dropped_records == 0 {
            return None;
        }
        let types_csv: String = dropped_types.iter().cloned().collect::<Vec<_>>().join(", ");
        let dropped_types_slot = if types_csv.is_empty() {
            String::new()
        } else {
            format!(" (dropped: {types_csv})")
        };
        Some(
            SCHEMA_FEEDBACK
                .replace("{dropped}", &dropped_records.to_string())
                .replace("{dropped_types}", &dropped_types_slot)
                .replace("{entity_types}", entity_types)
                .replace("{relationship_types}", rel_types),
        )
    }

    /// Drain one turn's response stream, concatenating assistant text (the
    /// delimiter records). Tool round-trips (Read/Grep) are handled inside the
    /// CLI; we collect the final text *and* count/log the tool calls the agent
    /// made, so the "self-serve context" mechanism is observable. Returns
    /// `(text, tool_use_count)`.
    async fn drain(&self, client: &ClaudeSdkClient) -> anyhow::Result<(String, usize)> {
        let mut stream = client
            .receive_response()
            .map_err(|e| anyhow::anyhow!("agentic receive failed: {e}"))?;
        let mut out = String::new();
        let mut tool_uses = 0usize;
        while let Some(msg) = stream.next().await {
            let msg = msg.map_err(|e| anyhow::anyhow!("agentic stream error: {e}"))?;
            if let SdkMessage::Assistant(a) = msg {
                for block in &a.content {
                    match block {
                        ContentBlock::Text(t) => out.push_str(&t.text),
                        ContentBlock::ToolUse(u) => {
                            tool_uses += 1;
                            if !self.quiet {
                                // Summarise the call with the most telling arg.
                                let arg = ["pattern", "file_path", "path", "query"]
                                    .iter()
                                    .find_map(|k| u.input.get(*k))
                                    .map(|v| v.to_string())
                                    .unwrap_or_default();
                                eprintln!("  [agent tool] {} {}", u.name, arg);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok((out.trim().to_string(), tool_uses))
    }
}

#[async_trait]
impl Extractor for AgenticExtractor {
    async fn extract(&self, text: &str) -> anyhow::Result<ExtractionResponse> {
        validate_input(text, self.config.min_segment_size, self.quiet)?;

        let mut slices = self.slices(text);
        // Lines are derivable here because we hold the full text; pre-chunked
        // input instead carries them from the chunk metadata.
        let doc_lines = {
            let line_index = crate::citation::LineIndex::new(text);
            for s in slices.iter_mut() {
                let (start, end) = line_index.line_range(s.start, s.end);
                s.set_line_span(start, end);
            }
            Some((1, line_index.total_lines()))
        };

        self.extract_slices(text, slices, doc_lines).await
    }

    /// Pre-chunked input: the given chunks ARE the slices — no re-slicing, no
    /// `min_segment_size` filtering. The on-disk `document.md` the agent can
    /// grep is reconstructed by joining the chunks; provenance lines come from
    /// the chunks' own metadata (chunks without it are simply not stamped).
    async fn extract_prechunked(&self, chunks: &[Segment]) -> anyhow::Result<ExtractionResponse> {
        if chunks.is_empty() {
            anyhow::bail!("No pre-chunked input provided");
        }
        let document = chunks
            .iter()
            .map(|s| s.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        // Whole-document provenance (used by the final gleaning pass) is the
        // overall span the chunks cover — when any of them knows its lines.
        let doc_lines = chunks
            .iter()
            .filter_map(Segment::line_span)
            .map(|line| (line.start as usize, line.end as usize))
            .fold(None, |acc: Option<(usize, usize)>, (s, e)| match acc {
                Some((lo, hi)) => Some((lo.min(s), hi.max(e))),
                None => Some((s, e)),
            });
        self.extract_slices(&document, chunks.to_vec(), doc_lines)
            .await
    }
}

impl AgenticExtractor {
    /// Shared session driver: set up the isolated read-only workspace with
    /// `document` on disk, then feed `slices` through one multi-turn session.
    async fn extract_slices(
        &self,
        document: &str,
        slices: Vec<Segment>,
        doc_lines: Option<(usize, usize)>,
    ) -> anyhow::Result<ExtractionResponse> {
        // 1. Isolated read-only workspace with the full document on disk.
        let (agent, env) = provider_env(&self.agent)?;
        let workdir: PathBuf =
            std::env::temp_dir().join(format!("kg-extract-{}", nanoid::nanoid!()));
        std::fs::create_dir_all(&workdir)
            .map_err(|e| anyhow::anyhow!("creating workspace {}: {e}", workdir.display()))?;
        let doc_path = workdir.join("document.md");
        std::fs::write(&doc_path, document)
            .map_err(|e| anyhow::anyhow!("writing {}: {e}", doc_path.display()))?;

        let result = self
            .run_session(slices, doc_lines, agent, env, &workdir)
            .await;

        // Best-effort cleanup of the temp workspace regardless of outcome.
        let _ = std::fs::remove_dir_all(&workdir);
        result
    }

    /// Build the final [`ExtractionResponse`] from a finished session's
    /// accumulators, and emit the per-mode summary line on stderr when not
    /// quiet. Pure over its inputs (the only side effect is the optional
    /// stderr summary, at the run's boundary) so the three policy arms' bookkeeping
    /// (Fixed drop counts, Evolving proposals, Off no-op) can be exercised
    /// without an SDK.
    fn assemble_response(
        outcome: SessionOutcome,
        policy: &SchemaPolicy,
        quiet: bool,
        mode: crate::types::SchemaMode,
        config: &ExtractionConfig,
    ) -> ExtractionResponse {
        let SessionOutcome {
            slices_count,
            total_tool_uses,
            entities,
            triples,
            parsed_results,
            total_dropped,
            dropped_types,
            new_nodes,
            new_relations,
        } = outcome;

        if !quiet {
            let tail = match policy {
                SchemaPolicy::Fixed(_) => {
                    format!(", {total_dropped} out-of-schema record(s) dropped")
                }
                SchemaPolicy::Evolving(_) => format!(
                    ", {} new type(s) proposed",
                    new_nodes.len() + new_relations.len()
                ),
                SchemaPolicy::Off => String::new(),
            };
            eprintln!(
                "agentic: {} slice(s), {} self-context tool call(s){}",
                slices_count, total_tool_uses, tail
            );
        }

        let mut kg = KnowledgeGraph::new();
        for e in entities.into_values() {
            kg.add_entity(e);
        }
        for t in triples {
            kg.add_triple(t);
        }

        let mut resp = ExtractionResponse::new(kg);
        resp.parsed_results = parsed_results;
        resp.config = Some(config.clone());
        resp.metadata
            .insert("tool_uses".into(), serde_json::json!(total_tool_uses));
        resp.metadata
            .insert("schema_mode".into(), serde_json::json!(mode.as_str()));
        match policy {
            SchemaPolicy::Fixed(_) => {
                resp.metadata.insert(
                    "schema_dropped_records".into(),
                    serde_json::json!(total_dropped),
                );
                resp.metadata.insert(
                    "schema_dropped_types".into(),
                    serde_json::json!(dropped_types.into_iter().collect::<Vec<_>>()),
                );
            }
            SchemaPolicy::Evolving(_) => {
                // Mirror SchemaJson/ToolCall's `new_schema_types` shape.
                resp.metadata.insert(
                    "new_schema_types".into(),
                    serde_json::json!({
                        "nodes": new_nodes.into_iter().collect::<Vec<_>>(),
                        "relations": new_relations.into_iter().collect::<Vec<_>>(),
                        "attributes": [],
                    }),
                );
            }
            SchemaPolicy::Off => {}
        }
        resp
    }

    async fn run_session(
        &self,
        slices: Vec<Segment>,
        _doc_lines: Option<(usize, usize)>,
        agent: String,
        env: std::collections::BTreeMap<String, String>,
        workdir: &Path,
    ) -> anyhow::Result<ExtractionResponse> {
        let entity_types = self.config.entity_types_list().join(", ");
        let rel_types = self.rel_types();
        // The schema policy decides the system-prompt tone and how per-slice
        // output is treated: Fixed validates+drops, Evolving records new types,
        // Off leaves it as hints. A non-empty schema is required for the two
        // constrained modes; an empty one degrades to Off.
        let policy = SchemaPolicy::for_mode(self.config.spec.mode, &self.config.spec.schema);
        if self.config.spec.mode.needs_schema()
            && matches!(policy, SchemaPolicy::Off)
            && !self.quiet
        {
            eprintln!(
                "agentic: --schema-mode {} but the schema is empty — treating as open (pass --schema <file>)",
                self.config.spec.mode.as_str()
            );
        }
        let schema_section = match policy {
            SchemaPolicy::Fixed(_) => SCHEMA_STRICT,
            SchemaPolicy::Evolving(_) => SCHEMA_EVOLVING,
            SchemaPolicy::Off => SCHEMA_HINTS,
        };
        let system = SYSTEM_PROMPT
            .replace("{schema_section}", schema_section)
            .replace("{entity_types}", &entity_types)
            .replace("{relationship_types}", &rel_types);

        let mut opts = ClaudeAgentOptions::default();
        opts.env.extend(env);
        opts.cwd = Some(workdir.to_path_buf());
        opts.system_prompt = Some(SystemPrompt::Text(system));
        // Read-only sandbox: the agent may pull context but cannot mutate.
        opts.allowed_tools = vec!["Read".into(), "Grep".into(), "Glob".into()];
        opts.disallowed_tools = vec![
            "Write".into(),
            "Edit".into(),
            "NotebookEdit".into(),
            "Bash".into(),
        ];
        opts.permission_mode = Some(PermissionMode::BypassPermissions);

        let mut client = ClaudeSdkClient::new(opts);
        client
            .connect(None)
            .await
            .map_err(|e| anyhow::anyhow!("agentic {agent} connect failed: {e}"))?;

        let n = slices.len();
        let mut all_entities: HashMap<String, Entity> = HashMap::new();
        let mut all_triples: Vec<Triple> = Vec::new();
        let mut parsed_results: Vec<ParsedResult> = Vec::new();
        let mut total_tool_uses = 0usize;
        // Schema bookkeeping: Fixed drops/feeds back; Evolving collects proposals.
        let mut pending_feedback: Option<String> = None;
        let mut total_dropped = 0usize;
        let mut all_dropped_types: BTreeSet<String> = BTreeSet::new();
        let mut new_nodes: BTreeSet<String> = BTreeSet::new();
        let mut new_relations: BTreeSet<String> = BTreeSet::new();

        // 2. Feed each slice as a turn in the same conversation.
        'slices: for (i, seg) in slices.iter().enumerate() {
            let base_prompt = Self::slice_prompt(i, n, &seg.content);
            // The first turn for this slice carries any correction the previous
            // slice's out-of-schema drops produced (re-anchors a drifting model).
            let mut prompt = match pending_feedback.take() {
                Some(fb) => format!("{fb}\n\n{base_prompt}"),
                None => base_prompt,
            };
            // One bounded re-do, fired only when a whole slice came back
            // entirely out-of-schema.
            let mut redo_left = 1u8;

            loop {
                if let Err(e) = client.query_default(prompt.clone()).await {
                    if !self.quiet {
                        eprintln!("agentic slice {} query error: {e}", i + 1);
                    }
                    break 'slices;
                }
                let output = match self.drain(&client).await {
                    Ok((o, tools)) => {
                        total_tool_uses += tools;
                        o
                    }
                    Err(e) => {
                        if !self.quiet {
                            eprintln!("agentic slice {} drain error: {e}", i + 1);
                        }
                        break 'slices;
                    }
                };
                if output.is_empty() {
                    continue 'slices;
                }
                let mut parsed = parse_output(&output, &self.config);
                if let Some(range) = seg.evidence_range() {
                    let cite = crate::citation::Citation::from_range(
                        self.config.source_doc.clone(),
                        range.clone(),
                    );
                    stamp_slice_citations(&mut parsed, &cite);
                }
                let parsed = parsed;

                let f = match &policy {
                    SchemaPolicy::Off => {
                        // Unconstrained (Open / empty schema): merge as-is.
                        commit_unconstrained_slice(
                            &mut all_entities,
                            &mut all_triples,
                            &mut parsed_results,
                            parsed,
                        );
                        continue 'slices;
                    }
                    SchemaPolicy::Evolving(f) => {
                        // Keep everything, but record the types the model used
                        // that lie outside the seed schema.
                        let tokens = entity_type_tokens(&output);
                        let (nn, nr) = f.new_types(&parsed.entities, &parsed.triples, &tokens);
                        if !self.quiet {
                            if let Some(line) = new_types_proposal_log(i, &nn, &nr) {
                                eprintln!("{line}");
                            }
                        }
                        new_nodes.extend(nn);
                        new_relations.extend(nr);
                        commit_unconstrained_slice(
                            &mut all_entities,
                            &mut all_triples,
                            &mut parsed_results,
                            parsed,
                        );
                        continue 'slices;
                    }
                    SchemaPolicy::Fixed(f) => f,
                };

                let tokens = entity_type_tokens(&output);
                let sf = f.apply(parsed.entities.clone(), parsed.triples.clone(), &tokens);

                // Degenerate case: the slice produced records but every one fell
                // outside the schema — re-do it once with a sterner reminder.
                if sf.all_dropped() && redo_left > 0 {
                    redo_left -= 1;
                    if !self.quiet {
                        eprintln!(
                            "  [schema] slice {} entirely out-of-schema — redoing",
                            i + 1
                        );
                    }
                    prompt = Self::redo_prompt(&entity_types, &rel_types);
                    continue; // re-do the SAME slice in this inner loop
                }

                // Commit the in-schema records.
                for (id, e) in sf.kept_entities {
                    merge_slice_entity(&mut all_entities, &id, &e);
                }
                all_triples.extend(sf.kept_triples);

                if sf.dropped_records > 0 {
                    total_dropped += sf.dropped_records;
                    all_dropped_types.extend(sf.dropped_types.iter().cloned());
                    let types_csv = sf
                        .dropped_types
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    if !self.quiet {
                        eprintln!(
                            "  [schema] slice {} dropped {} out-of-schema record(s){}",
                            i + 1,
                            sf.dropped_records,
                            if types_csv.is_empty() {
                                String::new()
                            } else {
                                format!(": {types_csv}")
                            }
                        );
                    }
                    pending_feedback = Self::drop_feedback(
                        sf.dropped_records,
                        &sf.dropped_types,
                        &entity_types,
                        &rel_types,
                    );
                }
                parsed_results.push(parsed);
                continue 'slices;
            }
        }

        // 3. Whole-graph relation gleaning: connect orphans across all slices.
        for _ in 0..self.max_relation_gleanings {
            let linked: HashSet<&str> = all_triples
                .iter()
                .flat_map(|t| [t.subject.id.as_str(), t.object.id.as_str()])
                .collect();
            let orphans: Vec<&Entity> = all_entities
                .values()
                .filter(|e| !linked.contains(e.id.as_str()))
                .collect();
            if orphans.is_empty() {
                break;
            }
            let orphan_list = orphans
                .iter()
                .map(|e| format!("- {}", e.label))
                .collect::<Vec<_>>()
                .join("\n");
            let prompt = RELATION_GLEANING_PROMPT
                .replace("{orphans}", &orphan_list)
                .replace("{relationship_types}", &rel_types);

            if client.query_default(prompt).await.is_err() {
                break;
            }
            let output = match self.drain(&client).await {
                Ok((o, tools)) => {
                    total_tool_uses += tools;
                    o
                }
                Err(_) => break,
            };
            if output.is_empty() || output.eq_ignore_ascii_case("no") {
                break;
            }
            let mut seen: HashSet<(String, String, String)> =
                all_triples.iter().map(|t| t.to_tuple()).collect();
            let rescued: Vec<Triple> = parse_relations_against(&output, &all_entities)
                .into_iter()
                .filter(|t| seen.insert(t.to_tuple()))
                // Fixed: gleaned relations must honour the schema too (endpoints
                // are already in-schema entities; only the relation type can stray).
                .filter(|t| match &policy {
                    SchemaPolicy::Fixed(f) => {
                        let rl = t
                            .predicate
                            .label
                            .clone()
                            .unwrap_or_else(|| t.predicate.predicate_type.to_string());
                        f.rel_ok(&norm_type(&rl))
                    }
                    _ => true,
                })
                .collect();
            // Evolving: gleaned relations are kept; record any out-of-seed types.
            if let SchemaPolicy::Evolving(f) = &policy {
                for t in &rescued {
                    let rl = t
                        .predicate
                        .label
                        .clone()
                        .unwrap_or_else(|| t.predicate.predicate_type.to_string());
                    let r = norm_type(&rl);
                    if !f.rel_ok(&r) {
                        new_relations.insert(r);
                    }
                }
            }
            if rescued.is_empty() {
                break;
            }
            // Gleaned relations come from a whole-document pass, so they cite
            // the full document rather than one slice (when its span is known).
            let rescued = match _doc_lines {
                Some((start_line, end_line)) => {
                    let mut rescued = rescued;
                    let cite = crate::citation::Citation::new(
                        self.config.source_doc.clone(),
                        start_line,
                        end_line,
                    );
                    for t in rescued.iter_mut() {
                        crate::citation::attach_citation(&mut t.metadata, &cite);
                    }
                    rescued
                }
                None => rescued,
            };
            all_triples.extend(rescued);
        }

        let _ = client.disconnect().await;

        let resp = Self::assemble_response(
            SessionOutcome {
                slices_count: n,
                total_tool_uses,
                entities: all_entities,
                triples: all_triples,
                parsed_results,
                total_dropped,
                dropped_types: all_dropped_types,
                new_nodes,
                new_relations,
            },
            &policy,
            self.quiet,
            self.config.spec.mode,
            &self.config,
        );
        Ok(resp)
    }
}

#[cfg(test)]
#[path = "agentic_test.rs"]
mod tests;
