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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use claude_agent_sdk_rs::types::SystemPrompt;
use claude_agent_sdk_rs::{
    ClaudeAgentOptions, ClaudeSdkClient, ContentBlock, Message as SdkMessage, PermissionMode,
};
use futures::StreamExt;

use super::{validate_input, Extractor, SchemaMode};
use crate::backend::sdk_agent::provider_env;
use crate::chunking::{segment, Segment};
use crate::extractor::simple::{entity_type_tokens, parse_output, parse_relations_against};
use crate::types::{
    Entity, ExtractionConfig, ExtractionResponse, KnowledgeGraph, ParsedResult, Schema, Triple,
};

const SYSTEM_PROMPT: &str = r#"You extract a knowledge graph (entities + relationships) from ONE document, slice by slice, across multiple turns.

The FULL document is saved at ./document.md in your working directory. When a slice is ambiguous — a pronoun, an abbreviation, a term defined elsewhere — use the Read or Grep tool on ./document.md to pull the surrounding context. Do not guess.

Across turns, REUSE the exact same entity_name for an entity you have already recorded (consistent coreference). Each turn, only add what is NEW.

Output a single `##`-separated list, each record in EXACTLY one of these forms:
(entity<|>entity_name<|>entity_type<|>a 10-20 word description<|>attributes)
(relationship<|>source_entity<|>target_entity<|>relationship_type<|>why they are related<|>strength_0_to_1)

DIRECTION RULE: a relationship must read left-to-right as a TRUE sentence: "source_entity relationship_type target_entity". EVERY type ending in _BY (FOUNDED_BY, DEVELOPED_BY, CREATED_BY, INVENTED_BY, PUBLISHED_BY, OWNED_BY, ...) is passive: the source is the thing acted on, the target is the doer — (relationship<|>Anthropic<|>Dario Amodei<|>FOUNDED_BY<|>...) is correct because "Anthropic [was] FOUNDED_BY Dario Amodei"; (relationship<|>Dario Amodei<|>Anthropic<|>FOUNDED_BY<|>...) is reversed and WRONG. A person is never the source of a *_BY relationship to their own work — for "X invented/created/wrote W" emit (W<|>X<|>INVENTED_BY) or the active (X<|>W<|>CREATED). Active types (FOUNDED, DEVELOPED, USES) point from the doer to the thing. Before emitting each relationship, read it aloud as a sentence; if it is false as written, swap source and target or pick the opposite-voice type.

Entity names capitalised, in the document's language. Output ONLY the records — no prose, no preamble.

{schema_section}"#;

/// Soft schema block (Open / Evolving / no-schema): types are hints only.
const SCHEMA_HINTS: &str = r#"Schema hints — Entity types: {entity_types}
Relationship types: {relationship_types}"#;

/// Hard schema block (Fixed): the model is told the closed type vocabulary up
/// front. Out-of-schema records are still validated and dropped on our side —
/// this just makes conformance the model's job, and the per-turn feedback
/// re-anchors it whenever it drifts.
const SCHEMA_STRICT: &str = r#"STRICT SCHEMA — you MUST use ONLY the types below. Any entity or relationship whose type is not in these lists is DISCARDED and wasted.
Entity types (use EXACTLY one of these as entity_type): {entity_types}
Relationship types (use EXACTLY one of these as relationship_type): {relationship_types}"#;

/// Open schema block (Evolving): the seed types are preferred, but the model
/// may coin a new type when none fits — those proposals are recorded (nothing
/// is dropped).
const SCHEMA_EVOLVING: &str = r#"SEED SCHEMA — PREFER these types, but you MAY introduce a NEW entity_type or relationship_type when the seed has no good fit. Nothing is discarded.
Entity types (seed): {entity_types}
Relationship types (seed): {relationship_types}"#;

/// Appended to the next slice's prompt after a turn dropped out-of-schema
/// records, so the model self-corrects mid-conversation.
const SCHEMA_FEEDBACK: &str = r#"NOTE: from your previous answer I discarded {dropped} record(s) because their types are NOT in the schema{dropped_types}. Stay strictly within — entity types: {entity_types}; relationship types: {relationship_types}. Do not emit any other type."#;

/// Sent to RE-DO a slice when *every* record it produced was out-of-schema
/// (the degenerate case): a single bounded retry with a sterner reminder.
const SCHEMA_REDO_PROMPT: &str = r#"EVERY record in your last answer used a type outside the schema, so all of it was discarded. Redo THIS slice using ONLY — entity types: {entity_types}; relationship types: {relationship_types}. Map each thing you found onto the closest allowed type; if something truly fits no allowed type, omit it. Output the records, or just NO if this slice has nothing that fits the schema."#;

const SLICE_PROMPT: &str = r#"Slice {i}/{n}:
<slice>
{slice}
</slice>
Extract entities and relationships from THIS slice, reusing names for entities you already recorded. Read/Grep ./document.md if you need context. Output the records, or just NO if this slice has none."#;

const RELATION_GLEANING_PROMPT: &str = r#"All slices are processed. The recorded entities below have NO relationship linking them to anything:
{orphans}

For EACH, look back over the WHOLE document (Read/Grep ./document.md if needed) and emit any relationship that connects it to another recorded entity; skip ones that genuinely stand alone — do NOT invent links.

Relationship types to prefer: {relationship_types}

Output ONLY relationships, `##`-separated, each as:
(relationship<|>source_entity<|>target_entity<|>relationship_type<|>why they are related<|>strength_0_to_1)
Each must read left-to-right as a true sentence ("source relationship_type target"); _BY types point from the thing acted on to the doer.
If none have any relationship, answer just: NO"#;

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
    /// [`SchemaMode::Fixed`] validates each slice and drops out-of-schema
    /// records (reminding the model on the next turn); [`SchemaMode::Evolving`]
    /// keeps everything but records the types used outside the seed as
    /// `new_schema_types`. [`SchemaMode::Open`] (and either constrained mode with
    /// an empty schema) leaves extraction unconstrained.
    pub fn schema_mode(mut self, mode: SchemaMode) -> Self {
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
                lines: None,
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

/// Normalize a type token to the canonical comparison form used for schema
/// matching: trimmed, uppercased, spaces/dashes → underscores.
fn norm_type(s: &str) -> String {
    s.trim().to_uppercase().replace([' ', '-'], "_")
}

/// What one slice's validation against a fixed schema produced.
struct SliceFilter {
    kept_entities: HashMap<String, Entity>,
    kept_triples: Vec<Triple>,
    dropped_types: BTreeSet<String>,
    dropped_records: usize,
}

impl SliceFilter {
    /// The slice yielded records but none survived the schema — the degenerate
    /// case that warrants a bounded re-do.
    fn all_dropped(&self) -> bool {
        self.dropped_records > 0 && self.kept_entities.is_empty() && self.kept_triples.is_empty()
    }
}

/// Closed-world validator for [`SchemaMode::Fixed`]: the allowed entity and
/// relation types, normalized for matching. An empty half (no node or no
/// relation constraint) lets everything through that half.
struct SchemaFilter {
    nodes: HashSet<String>,
    relations: HashSet<String>,
}

impl SchemaFilter {
    /// Build the seed-type sets, or `None` if the schema is empty (nothing to
    /// enforce or evolve from — the degenerate `Fixed`/`Evolving` cell that is
    /// just `Open`).
    fn build(schema: &Schema) -> Option<Self> {
        let nodes: HashSet<String> = schema.nodes.iter().map(|s| norm_type(s)).collect();
        let relations: HashSet<String> = schema.relations.iter().map(|s| norm_type(s)).collect();
        if nodes.is_empty() && relations.is_empty() {
            None
        } else {
            Some(SchemaFilter { nodes, relations })
        }
    }

    /// Collect the normalized entity/relation types that appear in a parse but
    /// are *outside* the seed schema — the `Evolving`-mode proposals. A seed half
    /// left empty is treated as unconstrained (nothing is "new" for it), mirroring
    /// the filter semantics.
    fn new_types(
        &self,
        entities: &HashMap<String, Entity>,
        triples: &[Triple],
        type_tokens: &HashMap<String, String>,
    ) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut nodes = BTreeSet::new();
        let mut relations = BTreeSet::new();
        for e in entities.values() {
            let raw = type_tokens
                .get(&e.label.trim().to_lowercase())
                .cloned()
                .unwrap_or_else(|| e.entity_type.to_string());
            let t = norm_type(&raw);
            if !self.node_ok(&t) {
                nodes.insert(t);
            }
        }
        for tr in triples {
            let rl = tr
                .predicate
                .label
                .clone()
                .unwrap_or_else(|| tr.predicate.predicate_type.to_string());
            let r = norm_type(&rl);
            if !self.rel_ok(&r) {
                relations.insert(r);
            }
        }
        (nodes, relations)
    }

    fn node_ok(&self, t: &str) -> bool {
        self.nodes.is_empty() || self.nodes.contains(t)
    }

    fn rel_ok(&self, t: &str) -> bool {
        self.relations.is_empty() || self.relations.contains(t)
    }

    /// Partition a slice's parse: drop entities whose type is out-of-schema and
    /// relations whose type is out-of-schema or whose endpoint was dropped.
    /// `type_tokens` carries each entity's *raw* type string (see
    /// [`entity_type_tokens`]) so domain-specific schema types — ones outside
    /// the known [`crate::types::EntityType`] vocabulary that the enum would
    /// collapse to `Other` — still match.
    fn apply(
        &self,
        entities: HashMap<String, Entity>,
        triples: Vec<Triple>,
        type_tokens: &HashMap<String, String>,
    ) -> SliceFilter {
        let mut kept_entities = HashMap::new();
        let mut dropped_types = BTreeSet::new();
        let mut dropped_records = 0usize;

        for (id, e) in entities {
            let raw = type_tokens
                .get(&e.label.trim().to_lowercase())
                .cloned()
                .unwrap_or_else(|| e.entity_type.to_string());
            let t = norm_type(&raw);
            if self.node_ok(&t) {
                kept_entities.insert(id, e);
            } else {
                dropped_records += 1;
                dropped_types.insert(t);
            }
        }

        let mut kept_triples = Vec::new();
        for tr in triples {
            let endpoints_ok = kept_entities.contains_key(&tr.subject.id)
                && kept_entities.contains_key(&tr.object.id);
            let rel_label = tr
                .predicate
                .label
                .clone()
                .unwrap_or_else(|| tr.predicate.predicate_type.to_string());
            let rel = norm_type(&rel_label);
            let rel_ok = self.rel_ok(&rel);
            if endpoints_ok && rel_ok {
                kept_triples.push(tr);
            } else {
                dropped_records += 1;
                if !rel_ok {
                    dropped_types.insert(rel);
                }
            }
        }

        SliceFilter {
            kept_entities,
            kept_triples,
            dropped_types,
            dropped_records,
        }
    }
}

/// How the seed schema governs a run, resolved from [`SchemaMode`] + schema.
enum SchemaPolicy {
    /// Unconstrained: `Open`, or `Fixed`/`Evolving` with an empty schema.
    Off,
    /// Closed-world: validate each slice, drop out-of-schema, feed back.
    Fixed(SchemaFilter),
    /// Seeded but open: keep everything, record types outside the seed.
    Evolving(SchemaFilter),
}

impl SchemaPolicy {
    fn for_mode(mode: SchemaMode, schema: &Schema) -> Self {
        match mode {
            SchemaMode::Open => SchemaPolicy::Off,
            SchemaMode::Fixed => SchemaFilter::build(schema)
                .map(SchemaPolicy::Fixed)
                .unwrap_or(SchemaPolicy::Off),
            SchemaMode::Evolving => SchemaFilter::build(schema)
                .map(SchemaPolicy::Evolving)
                .unwrap_or(SchemaPolicy::Off),
        }
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
                s.lines = Some(line_index.line_range(s.start, s.end));
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
        let doc_lines = chunks.iter().filter_map(|s| s.lines).fold(
            None,
            |acc: Option<(usize, usize)>, (s, e)| match acc {
                Some((lo, hi)) => Some((lo.min(s), hi.max(e))),
                None => Some((s, e)),
            },
        );
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
            let base_prompt = SLICE_PROMPT
                .replace("{i}", &(i + 1).to_string())
                .replace("{n}", &n.to_string())
                .replace("{slice}", &seg.content);
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
                if let Some((start_line, end_line)) = seg.lines {
                    let cite = crate::citation::Citation::new(
                        self.config.source_doc.clone(),
                        start_line,
                        end_line,
                    );
                    for e in parsed.entities.values_mut() {
                        crate::citation::attach_citation(&mut e.metadata, &cite);
                    }
                    // Endpoint snapshots too: `add_triple` re-inserts them into
                    // the entity table, where an unstamped copy would erase the
                    // entity's provenance (it unions, but only what's present).
                    for t in parsed.triples.iter_mut() {
                        crate::citation::attach_citation(&mut t.metadata, &cite);
                        crate::citation::attach_citation(&mut t.subject.metadata, &cite);
                        crate::citation::attach_citation(&mut t.object.metadata, &cite);
                    }
                }
                let parsed = parsed;

                let f = match &policy {
                    SchemaPolicy::Off => {
                        // Unconstrained (Open / empty schema): merge as-is.
                        for (id, e) in &parsed.entities {
                            merge_slice_entity(&mut all_entities, id, e);
                        }
                        all_triples.extend(parsed.triples.clone());
                        parsed_results.push(parsed);
                        continue 'slices;
                    }
                    SchemaPolicy::Evolving(f) => {
                        // Keep everything, but record the types the model used
                        // that lie outside the seed schema.
                        let tokens = entity_type_tokens(&output);
                        let (nn, nr) = f.new_types(&parsed.entities, &parsed.triples, &tokens);
                        if !self.quiet && (!nn.is_empty() || !nr.is_empty()) {
                            let csv = nn
                                .iter()
                                .chain(nr.iter())
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ");
                            eprintln!(
                                "  [schema] slice {} proposed {} new type(s): {csv}",
                                i + 1,
                                nn.len() + nr.len()
                            );
                        }
                        new_nodes.extend(nn);
                        new_relations.extend(nr);
                        for (id, e) in &parsed.entities {
                            merge_slice_entity(&mut all_entities, id, e);
                        }
                        all_triples.extend(parsed.triples.clone());
                        parsed_results.push(parsed);
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
                    prompt = SCHEMA_REDO_PROMPT
                        .replace("{entity_types}", &entity_types)
                        .replace("{relationship_types}", &rel_types);
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
                    let dropped_types = if types_csv.is_empty() {
                        String::new()
                    } else {
                        format!(" (dropped: {types_csv})")
                    };
                    pending_feedback = Some(
                        SCHEMA_FEEDBACK
                            .replace("{dropped}", &sf.dropped_records.to_string())
                            .replace("{dropped_types}", &dropped_types)
                            .replace("{entity_types}", &entity_types)
                            .replace("{relationship_types}", &rel_types),
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

        let mut kg = KnowledgeGraph::new();
        for e in all_entities.into_values() {
            kg.add_entity(e);
        }
        for t in all_triples {
            kg.add_triple(t);
        }
        if !self.quiet {
            let tail = match &policy {
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
                n, total_tool_uses, tail
            );
        }
        let mut resp = ExtractionResponse::new(kg);
        resp.parsed_results = parsed_results;
        resp.config = Some(self.config.clone());
        resp.metadata
            .insert("tool_uses".into(), serde_json::json!(total_tool_uses));
        resp.metadata.insert(
            "schema_mode".into(),
            serde_json::json!(self.config.spec.mode.as_str()),
        );
        match &policy {
            SchemaPolicy::Fixed(_) => {
                resp.metadata.insert(
                    "schema_dropped_records".into(),
                    serde_json::json!(total_dropped),
                );
                resp.metadata.insert(
                    "schema_dropped_types".into(),
                    serde_json::json!(all_dropped_types.into_iter().collect::<Vec<_>>()),
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
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EntityType, Predicate, PredicateType};

    fn ent(id: &str, label: &str, ty: EntityType) -> Entity {
        Entity::new(id, label, ty)
    }

    fn tokens(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(n, t)| (n.to_lowercase(), t.to_string()))
            .collect()
    }

    #[test]
    fn policy_dispatch_by_mode_and_schema() {
        let schema = Schema::new(vec!["PRODUCT".into()], vec![], vec![]);
        assert!(matches!(
            SchemaPolicy::for_mode(SchemaMode::Open, &schema),
            SchemaPolicy::Off
        ));
        assert!(matches!(
            SchemaPolicy::for_mode(SchemaMode::Fixed, &schema),
            SchemaPolicy::Fixed(_)
        ));
        assert!(matches!(
            SchemaPolicy::for_mode(SchemaMode::Evolving, &schema),
            SchemaPolicy::Evolving(_)
        ));
        // Empty schema degrades the constrained modes to Off.
        assert!(matches!(
            SchemaPolicy::for_mode(SchemaMode::Fixed, &Schema::default()),
            SchemaPolicy::Off
        ));
        assert!(matches!(
            SchemaPolicy::for_mode(SchemaMode::Evolving, &Schema::default()),
            SchemaPolicy::Off
        ));
    }

    #[test]
    fn drops_out_of_schema_entity_and_its_dependent_relation() {
        // Only PRODUCT entities and USES relations are allowed.
        let schema = Schema::new(vec!["PRODUCT".into()], vec!["USES".into()], vec![]);
        let f = SchemaFilter::build(&schema).unwrap();

        let mut entities = HashMap::new();
        entities.insert("p".to_string(), ent("p", "Widget", EntityType::Product));
        entities.insert("o".to_string(), ent("o", "Acme", EntityType::Organization));

        // USES is allowed, but the object entity is dropped → relation must go too.
        let t = Triple::new(
            ent("p", "Widget", EntityType::Product),
            Predicate::with_label(PredicateType::Uses, "USES"),
            ent("o", "Acme", EntityType::Organization),
        );

        let toks = tokens(&[("widget", "PRODUCT"), ("acme", "ORGANIZATION")]);
        let sf = f.apply(entities, vec![t], &toks);

        assert_eq!(sf.kept_entities.len(), 1);
        assert!(sf.kept_entities.contains_key("p"));
        assert!(
            sf.kept_triples.is_empty(),
            "relation to a dropped endpoint must be dropped"
        );
        assert!(sf.dropped_types.contains("ORGANIZATION"));
        assert_eq!(sf.dropped_records, 2, "one entity + one relation");
        assert!(!sf.all_dropped(), "a product survived");
    }

    #[test]
    fn drops_out_of_schema_relation_type_but_keeps_entities() {
        let schema = Schema::new(vec!["PRODUCT".into()], vec!["USES".into()], vec![]);
        let f = SchemaFilter::build(&schema).unwrap();
        let mut entities = HashMap::new();
        entities.insert("a".into(), ent("a", "Widget", EntityType::Product));
        entities.insert("b".into(), ent("b", "Gadget", EntityType::Product));
        // Both endpoints in-schema, but DEPENDS_ON is not an allowed relation.
        let t = Triple::new(
            ent("a", "Widget", EntityType::Product),
            Predicate::with_label(PredicateType::RelatedTo, "DEPENDS_ON"),
            ent("b", "Gadget", EntityType::Product),
        );
        let toks = tokens(&[("widget", "PRODUCT"), ("gadget", "PRODUCT")]);
        let sf = f.apply(entities, vec![t], &toks);
        assert_eq!(sf.kept_entities.len(), 2);
        assert!(sf.kept_triples.is_empty());
        assert!(sf.dropped_types.contains("DEPENDS_ON"));
    }

    #[test]
    fn custom_schema_type_matches_via_raw_token() {
        // GADGET is not an EntityType variant — from_loose collapses it to Other,
        // so enum-level checking would wrongly drop it. The raw token rescues it.
        let schema = Schema::new(vec!["GADGET".into()], vec![], vec![]);
        let f = SchemaFilter::build(&schema).unwrap();
        let mut entities = HashMap::new();
        entities.insert("g".into(), ent("g", "Widget X", EntityType::Other));
        let toks = tokens(&[("widget x", "GADGET")]);
        let sf = f.apply(entities, vec![], &toks);
        assert_eq!(
            sf.kept_entities.len(),
            1,
            "custom type must match by raw token"
        );
        assert_eq!(sf.dropped_records, 0);
    }

    #[test]
    fn all_dropped_flags_the_degenerate_slice() {
        let schema = Schema::new(vec!["PRODUCT".into()], vec![], vec![]);
        let f = SchemaFilter::build(&schema).unwrap();
        let mut entities = HashMap::new();
        entities.insert("o".into(), ent("o", "Acme", EntityType::Organization));
        let toks = tokens(&[("acme", "ORGANIZATION")]);
        let sf = f.apply(entities, vec![], &toks);
        assert!(sf.all_dropped(), "every record fell outside the schema");
    }

    #[test]
    fn evolving_collects_types_outside_the_seed() {
        // Seed allows PRODUCT entities and USES relations. The model also emits an
        // ORGANIZATION entity and a DEPENDS_ON relation — Evolving keeps both but
        // reports them as proposed new types (nothing dropped).
        let schema = Schema::new(vec!["PRODUCT".into()], vec!["USES".into()], vec![]);
        let f = SchemaFilter::build(&schema).unwrap();
        let mut entities = HashMap::new();
        entities.insert("p".into(), ent("p", "Widget", EntityType::Product));
        entities.insert("o".into(), ent("o", "Acme", EntityType::Organization));
        let t = Triple::new(
            ent("p", "Widget", EntityType::Product),
            Predicate::with_label(PredicateType::RelatedTo, "DEPENDS_ON"),
            ent("o", "Acme", EntityType::Organization),
        );
        let toks = tokens(&[("widget", "PRODUCT"), ("acme", "ORGANIZATION")]);
        let (nodes, relations) = f.new_types(&entities, &[t], &toks);
        assert!(
            nodes.contains("ORGANIZATION"),
            "out-of-seed node must be proposed"
        );
        assert!(!nodes.contains("PRODUCT"), "seed node is not a proposal");
        assert!(
            relations.contains("DEPENDS_ON"),
            "out-of-seed relation must be proposed"
        );
        assert!(
            !relations.contains("USES"),
            "seed relation is not a proposal"
        );
    }
}
