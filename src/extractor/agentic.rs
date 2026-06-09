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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use claude_agent_sdk_rs::types::SystemPrompt;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeSdkClient, ContentBlock, Message as SdkMessage, PermissionMode};
use futures::StreamExt;

use super::{validate_input, Extractor};
use crate::backend::sdk_agent::provider_env;
use crate::chunking::segment;
use crate::extractor::simple::{parse_output, parse_relations_against};
use crate::types::{Entity, ExtractionConfig, ExtractionResponse, KnowledgeGraph, ParsedResult, Triple};

const SYSTEM_PROMPT: &str = r#"You extract a knowledge graph (entities + relationships) from ONE document, slice by slice, across multiple turns.

The FULL document is saved at ./document.md in your working directory. When a slice is ambiguous — a pronoun, an abbreviation, a term defined elsewhere — use the Read or Grep tool on ./document.md to pull the surrounding context. Do not guess.

Across turns, REUSE the exact same entity_name for an entity you have already recorded (consistent coreference). Each turn, only add what is NEW.

Output a single `##`-separated list, each record in EXACTLY one of these forms:
(entity<|>entity_name<|>entity_type<|>a 10-20 word description<|>attributes)
(relationship<|>source_entity<|>target_entity<|>relationship_type<|>why they are related<|>strength_0_to_1)

Entity names capitalised, in the document's language. Output ONLY the records — no prose, no preamble.

Schema hints — Entity types: {entity_types}
Relationship types: {relationship_types}"#;

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
If none have any relationship, answer just: NO"#;

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
        AgenticExtractor { config, agent: agent.to_string(), max_relation_gleanings: 0, quiet: false }
    }

    /// Enable whole-graph relation-gleaning with `n` rounds (builder style).
    pub fn relation_gleanings(mut self, n: usize) -> Self {
        self.max_relation_gleanings = n;
        self
    }

    /// Slice the text the same way the Simple engine does: a single slice when
    /// the text fits in `segment_size`, otherwise segmented (dropping a tiny
    /// trailing fragment).
    fn slices(&self, text: &str) -> Vec<String> {
        if text.chars().count() > self.config.segment_size {
            segment(text, self.config.chunker, self.config.segment_size, self.config.overlap)
                .into_iter()
                .enumerate()
                .filter(|(i, s)| !(s.content.chars().count() < self.config.min_segment_size && *i > 0))
                .map(|(_, s)| s.content)
                .collect()
        } else {
            vec![text.to_string()]
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

#[async_trait]
impl Extractor for AgenticExtractor {
    async fn extract(&self, text: &str) -> anyhow::Result<ExtractionResponse> {
        validate_input(text, self.config.min_segment_size, self.quiet)?;

        // 1. Isolated read-only workspace with the full document on disk.
        let (agent, env) = provider_env(&self.agent)?;
        let workdir: PathBuf = std::env::temp_dir().join(format!("kg-extract-{}", nanoid::nanoid!()));
        std::fs::create_dir_all(&workdir)
            .map_err(|e| anyhow::anyhow!("creating workspace {}: {e}", workdir.display()))?;
        let doc_path = workdir.join("document.md");
        std::fs::write(&doc_path, text)
            .map_err(|e| anyhow::anyhow!("writing {}: {e}", doc_path.display()))?;

        let result = self.run_session(text, agent, env, &workdir).await;

        // Best-effort cleanup of the temp workspace regardless of outcome.
        let _ = std::fs::remove_dir_all(&workdir);
        result
    }
}

impl AgenticExtractor {
    async fn run_session(
        &self,
        text: &str,
        agent: String,
        env: std::collections::BTreeMap<String, String>,
        workdir: &Path,
    ) -> anyhow::Result<ExtractionResponse> {
        let entity_types = self.config.entity_types_list().join(", ");
        let rel_types = self.rel_types();
        let system = SYSTEM_PROMPT
            .replace("{entity_types}", &entity_types)
            .replace("{relationship_types}", &rel_types);

        let mut opts = ClaudeAgentOptions::default();
        opts.env.extend(env);
        opts.cwd = Some(workdir.to_path_buf());
        opts.system_prompt = Some(SystemPrompt::Text(system));
        // Read-only sandbox: the agent may pull context but cannot mutate.
        opts.allowed_tools = vec!["Read".into(), "Grep".into(), "Glob".into()];
        opts.disallowed_tools =
            vec!["Write".into(), "Edit".into(), "NotebookEdit".into(), "Bash".into()];
        opts.permission_mode = Some(PermissionMode::BypassPermissions);

        let mut client = ClaudeSdkClient::new(opts);
        client
            .connect(None)
            .await
            .map_err(|e| anyhow::anyhow!("agentic {agent} connect failed: {e}"))?;

        let slices = self.slices(text);
        let n = slices.len();
        let mut all_entities: HashMap<String, Entity> = HashMap::new();
        let mut all_triples: Vec<Triple> = Vec::new();
        let mut parsed_results: Vec<ParsedResult> = Vec::new();
        let mut total_tool_uses = 0usize;

        // 2. Feed each slice as a turn in the same conversation.
        for (i, slice) in slices.iter().enumerate() {
            let prompt = SLICE_PROMPT
                .replace("{i}", &(i + 1).to_string())
                .replace("{n}", &n.to_string())
                .replace("{slice}", slice);
            if let Err(e) = client.query_default(prompt).await {
                if !self.quiet {
                    eprintln!("agentic slice {} query error: {e}", i + 1);
                }
                break;
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
                    break;
                }
            };
            if output.is_empty() {
                continue;
            }
            let parsed = parse_output(&output, &self.config);
            for (id, e) in &parsed.entities {
                all_entities.entry(id.clone()).or_insert_with(|| e.clone());
            }
            all_triples.extend(parsed.triples.clone());
            parsed_results.push(parsed);
        }

        // 3. Whole-graph relation gleaning: connect orphans across all slices.
        for _ in 0..self.max_relation_gleanings {
            let linked: HashSet<&str> = all_triples
                .iter()
                .flat_map(|t| [t.subject.id.as_str(), t.object.id.as_str()])
                .collect();
            let orphans: Vec<&Entity> =
                all_entities.values().filter(|e| !linked.contains(e.id.as_str())).collect();
            if orphans.is_empty() {
                break;
            }
            let orphan_list =
                orphans.iter().map(|e| format!("- {}", e.label)).collect::<Vec<_>>().join("\n");
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
                .collect();
            if rescued.is_empty() {
                break;
            }
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
            eprintln!("agentic: {} slice(s), {} self-context tool call(s)", n, total_tool_uses);
        }
        let mut resp = ExtractionResponse::new(kg);
        resp.parsed_results = parsed_results;
        resp.config = Some(self.config.clone());
        resp.metadata.insert("tool_uses".into(), serde_json::json!(total_tool_uses));
        Ok(resp)
    }
}
