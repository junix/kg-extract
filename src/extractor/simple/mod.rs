//! SimpleExtractor — general LLM chat extractor with GraphRAG-style delimiter
//! prompting and multi-gleaning (ported from `graph/kg_extractor/simple.py`).
//!
//! Module layout:
//! - [`prompts`] — the delimiter-prompt templates.
//! - [`parse`] — delimiter-output parsing into entities + triples.
//! - this file — the [`SimpleExtractor`] struct, its [`Extractor`] impl, and the
//!   per-chunk extraction core (segmentation, multi-gleaning, merging).

mod parse;
mod prompts;

pub(crate) use parse::{entity_type_tokens, parse_output, parse_relations_against};

use super::{validate_input, Extractor};
use crate::backend::{ChatSession, CompletionOptions, LlmBackend, ReplaySession};
use crate::chunking::{segment, Segment};
use crate::merger::{merge_all, merge_all_dedup_coref, merge_all_dedup_llm};
use crate::types::{ExtractionConfig, ExtractionResponse, KnowledgeGraph, ParsedResult};

#[cfg(test)]
use crate::types::PredicateType;
use futures::stream::{self, StreamExt};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub const MAX_GLEANINGS: usize = 2;
const TUPLE_DELIMITER: &str = "<|>";
const RECORD_DELIMITER: &str = "##";

use prompts::{CONTINUE_PROMPT, EXTRACTION_PROMPT, RELATION_GLEANING_PROMPT, SYSTEM_PROMPT};

/// Knowledge graph extractor using a general LLM chat interface.
pub struct SimpleExtractor {
    backend: Arc<dyn LlmBackend>,
    config: ExtractionConfig,
    pub quiet: bool,
    pub max_gleanings: usize,
    /// Targeted relation-gleaning rounds run *after* entity gleaning: each round
    /// re-questions the chunk's orphan entities (no incident edge) to recover
    /// their relationships. `0` (default) preserves the Python port's behaviour.
    pub max_relation_gleanings: usize,
    pub context: String,
}

impl SimpleExtractor {
    /// Default config: `qwen-max`, segment_size 5000.
    pub fn default_config() -> ExtractionConfig {
        ExtractionConfig {
            model_name: "qwen-max".into(),
            segment_size: 5000,
            min_segment_size: 100,
            ..Default::default()
        }
    }

    pub fn new(backend: Arc<dyn LlmBackend>) -> Self {
        SimpleExtractor {
            backend,
            config: Self::default_config(),
            quiet: false,
            max_gleanings: MAX_GLEANINGS,
            max_relation_gleanings: 0,
            context: String::new(),
        }
    }

    pub fn with_config(backend: Arc<dyn LlmBackend>, config: ExtractionConfig) -> Self {
        SimpleExtractor {
            backend,
            config,
            quiet: false,
            max_gleanings: MAX_GLEANINGS,
            max_relation_gleanings: 0,
            context: String::new(),
        }
    }

    /// Enable targeted relation-gleaning with `n` rescue rounds (builder style).
    pub fn relation_gleanings(mut self, n: usize) -> Self {
        self.max_relation_gleanings = n;
        self
    }

    pub fn config(&self) -> &ExtractionConfig {
        &self.config
    }

    /// Extract one chunk via the GraphRAG prompt + multi-gleaning loop, building
    /// that chunk's (un-deduped) graph. Multiple chunks run this concurrently.
    async fn extract_chunk(&self, chunk: &str) -> (Vec<ParsedResult>, KnowledgeGraph) {
        let entity_types = self.config.entity_types_list().join(", ");
        let rel_types = if self.config.predicates_list().is_empty() {
            "related_to, part_of, uses".to_string()
        } else {
            self.config.predicates_list().join(", ")
        };
        let attr_types = if self.config.attributes_list().is_empty() {
            "name, type, description".to_string()
        } else {
            self.config.attributes_list().join(", ")
        };

        let extraction_prompt = EXTRACTION_PROMPT
            .replace("{entity_types}", &entity_types)
            .replace("{relationship_types}", &rel_types)
            .replace("{attribute_types}", &attr_types)
            .replace("{context}", &self.context)
            .replace("{chunk}", chunk);

        let opts = CompletionOptions {
            model: self.config.model_name.clone(),
            temperature: 0.3,
            max_tokens: 6500,
        };

        // Drive the whole chunk through ONE multi-turn session: native for
        // backends with a real conversation protocol (the SDK retains context
        // across turns), or a history-replaying fallback for the rest. Both
        // expose the same `send` contract, so the gleaning logic below is
        // transport-agnostic.
        let system = SYSTEM_PROMPT.to_string();
        let mut session: Box<dyn ChatSession> =
            match self.backend.open_session(Some(system.clone()), &opts).await {
                Ok(Some(s)) => s,
                Ok(None) => Box::new(ReplaySession::new(
                    self.backend.clone(),
                    Some(system),
                    opts.clone(),
                )),
                Err(e) => {
                    if !self.quiet {
                        eprintln!("Session open error: {e}");
                    }
                    return (Vec::new(), KnowledgeGraph::new());
                }
            };

        let mut all_entities: HashMap<String, crate::types::Entity> = HashMap::new();
        let mut all_triples: Vec<crate::types::Triple> = Vec::new();
        let mut parsed_results: Vec<ParsedResult> = Vec::new();

        // Entity gleaning: the extraction turn, then "what did you miss?" turns.
        for i in 0..=self.max_gleanings {
            let prompt: &str = if i == 0 {
                &extraction_prompt
            } else {
                CONTINUE_PROMPT
            };
            let output = match session.send(prompt).await {
                Ok(o) => o.trim().to_string(),
                Err(e) => {
                    if !self.quiet {
                        eprintln!("Extraction error: {e}");
                    }
                    break;
                }
            };
            if output.is_empty() {
                continue;
            }

            let parsed = parse_output(&output, &self.config);
            let new_entities = parsed.entities.len();
            let new_triples = parsed.triples.len();
            for (id, e) in &parsed.entities {
                all_entities.entry(id.clone()).or_insert_with(|| e.clone());
            }
            all_triples.extend(parsed.triples.clone());
            parsed_results.push(parsed);

            // Early stop on a gleaning round that added nothing.
            if i > 0 && new_entities == 0 && new_triples == 0 {
                break;
            }
        }

        // Relation gleaning: name the entities that ended up with no edge and ask
        // the model — in the SAME session, so the source text is still in context
        // — to connect them. Each round only adds edges between already-known
        // entities, so it shrinks the orphan set without inventing dangling nouns.
        let rescue_rel_types = if self.config.predicates_list().is_empty() {
            "related_to, part_of, uses, produces, has_property".to_string()
        } else {
            self.config.predicates_list().join(", ")
        };
        for _ in 0..self.max_relation_gleanings {
            let linked: HashSet<&str> = all_triples
                .iter()
                .flat_map(|t| [t.subject.id.as_str(), t.object.id.as_str()])
                .collect();
            let orphans: Vec<&crate::types::Entity> = all_entities
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
                .replace("{relationship_types}", &rescue_rel_types);

            let output = match session.send(&prompt).await {
                Ok(o) => o.trim().to_string(),
                Err(e) => {
                    if !self.quiet {
                        eprintln!("Relation-gleaning error: {e}");
                    }
                    break;
                }
            };
            if output.is_empty() || output.eq_ignore_ascii_case("no") {
                break;
            }

            // Keep only edges that resolve to known entities and are genuinely new.
            let mut seen: HashSet<(String, String, String)> =
                all_triples.iter().map(|t| t.to_tuple()).collect();
            let rescued: Vec<crate::types::Triple> =
                parse_relations_against(&output, &all_entities)
                    .into_iter()
                    .filter(|t| seen.insert(t.to_tuple()))
                    .collect();
            if rescued.is_empty() {
                break;
            }
            all_triples.extend(rescued);
        }

        if let Err(e) = session.finish().await {
            if !self.quiet {
                eprintln!("Session finish error: {e}");
            }
        }

        let mut kg = KnowledgeGraph::new();
        for e in all_entities.into_values() {
            kg.add_entity(e);
        }
        for t in all_triples {
            kg.add_triple(t);
        }
        (parsed_results, kg)
    }
}

#[async_trait::async_trait]
impl Extractor for SimpleExtractor {
    async fn extract(&self, text: &str) -> anyhow::Result<ExtractionResponse> {
        validate_input(text, self.config.min_segment_size, self.quiet)?;

        // Segment only when the text exceeds segment_size; otherwise a single
        // chunk preserves the original single-shot + gleaning behaviour exactly.
        // Each chunk keeps its char offsets so provenance can be line-mapped.
        #[allow(unused_mut)]
        let mut chunks: Vec<Segment> = if text.chars().count() > self.config.segment_size {
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
        };

        // Lines are derivable here because we hold the full text; pre-chunked
        // input instead carries them from the chunk metadata.
        {
            let line_index = crate::citation::LineIndex::new(text);
            for c in chunks.iter_mut() {
                c.lines = Some(line_index.line_range(c.start, c.end));
            }
        }

        self.extract_segments(chunks).await
    }

    /// Pre-chunked input: the given chunks ARE the segments — no re-chunking,
    /// no `min_segment_size` filtering. Provenance lines come from the chunks'
    /// own metadata (chunks without it are simply not stamped).
    async fn extract_prechunked(&self, chunks: &[Segment]) -> anyhow::Result<ExtractionResponse> {
        if chunks.is_empty() {
            anyhow::bail!("No pre-chunked input provided");
        }
        self.extract_segments(chunks.to_vec()).await
    }
}

impl SimpleExtractor {
    /// Shared extraction core: run every segment through the per-chunk
    /// prompt + gleaning pipeline concurrently, stamp provenance, and fold the
    /// per-chunk graphs per the configured dedup strategy.
    async fn extract_segments(&self, chunks: Vec<Segment>) -> anyhow::Result<ExtractionResponse> {
        // Extract chunks concurrently. LLM calls are I/O-bound, so `buffered`
        // runs up to `max_concurrency` in flight while preserving chunk order.
        let max_conc = self.config.max_concurrency.max(1);
        let per_chunk: Vec<(Vec<ParsedResult>, KnowledgeGraph)> = stream::iter(chunks)
            .map(|seg| async move {
                let result = self.extract_chunk(&seg.content).await;
                match seg.lines {
                    Some((start_line, end_line)) => {
                        let (prs, mut kg) = result;
                        let cite = crate::citation::Citation::new(
                            self.config.source_doc.clone(),
                            start_line,
                            end_line,
                        );
                        crate::citation::stamp_graph(&mut kg, &cite);
                        (prs, kg)
                    }
                    None => result,
                }
            })
            .buffered(max_conc)
            .collect()
            .await;

        let mut parsed_results: Vec<ParsedResult> = Vec::new();
        let mut graphs: Vec<KnowledgeGraph> = Vec::new();
        for (prs, kg) in per_chunk {
            parsed_results.extend(prs);
            graphs.push(kg);
        }

        // Fold the per-chunk graphs, deduplicating per the configured strategy.
        let kg = if self.config.spec.merge_duplicates {
            let strategy = self.config.spec.merge_strategy;
            if strategy.needs_backend() {
                let opts = CompletionOptions {
                    model: self.config.model_name.clone(),
                    temperature: 0.3,
                    max_tokens: 1000,
                };
                merge_all_dedup_llm(graphs, &self.backend, &opts).await
            } else {
                merge_all_dedup_coref(graphs, strategy, self.config.spec.coref)
            }
        } else {
            merge_all(graphs)
        };

        let mut resp = ExtractionResponse::new(kg);
        resp.parsed_results = parsed_results;
        resp.config = Some(self.config.clone());
        Ok(resp)
    }
}

#[cfg(test)]
#[path = "simple_test.rs"]
mod tests;
