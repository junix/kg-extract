//! TriplexExtractor — NER + triple extraction via a Triplex-style model
//! (default `sciphi/triplex:latest` on Ollama), segmenting large inputs.
//! Ported from `graph/kg_extractor/triplex.py`.

use super::{validate_input, Extractor};
use crate::backend::{CompletionOptions, LlmBackend};
use crate::chunking::{segment, Segment};
use crate::merger::{merge_all, merge_knowledge_graphs};
use crate::parser::{
    create_entities_from_parsed, create_triples_from_parsed, extract_json_from_response,
    parse_entities_and_triples,
};
use crate::types::{ExtractionConfig, ExtractionResponse, KnowledgeGraph, ParsedResult};
use std::collections::HashMap;
use std::sync::Arc;

const TEMPLATE: &str = r#"Perform Named Entity Recognition (NER) and extract knowledge graph triplets from the text according to the provided schema. NER identifies named entities of given entity types, and triple extraction identifies relationships between entities using specified predicates.

**Schema Definition:**
Entity Types: {entity_types}
Relationship Types: {predicates}
Attribute Types: {attributes}

**Instructions:**
1. Extract entities with their types and attributes (if applicable)
2. Extract relationships between entities using the specified relationship types
3. Include entity attributes when relevant from the attribute types list

**Text:**
{text}
"#;

/// Knowledge graph extractor using a Triplex-style model.
pub struct TriplexExtractor {
    backend: Arc<dyn LlmBackend>,
    config: ExtractionConfig,
    pub quiet: bool,
}

impl TriplexExtractor {
    /// Default config: `sciphi/triplex:latest`, segment_size 3000.
    pub fn default_config() -> ExtractionConfig {
        ExtractionConfig {
            model_name: "sciphi/triplex:latest".into(),
            segment_size: 3000,
            min_segment_size: 100,
            ..Default::default()
        }
    }

    pub fn new(backend: Arc<dyn LlmBackend>) -> Self {
        TriplexExtractor { backend, config: Self::default_config(), quiet: false }
    }

    pub fn with_config(backend: Arc<dyn LlmBackend>, config: ExtractionConfig) -> Self {
        TriplexExtractor { backend, config, quiet: false }
    }

    pub fn config(&self) -> &ExtractionConfig {
        &self.config
    }

    async fn extract_segment(&self, text: &str) -> Option<(ParsedResult, KnowledgeGraph)> {
        let attrs = if self.config.attributes_list().is_empty() {
            "name, type, description".to_string()
        } else {
            self.config.attributes_list().join(", ")
        };
        let prompt = TEMPLATE
            .replace("{entity_types}", &self.config.entity_types_list().join(", "))
            .replace("{predicates}", &self.config.predicates_list().join(", "))
            .replace("{attributes}", &attrs)
            .replace("{text}", text);

        let opts = CompletionOptions {
            model: self.config.model_name.clone(),
            temperature: 0.0,
            max_tokens: 4000,
        };
        let response = match self.backend.complete_prompt(&prompt, &opts).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Extraction failed for segment: {e}");
                return None;
            }
        };

        let json = extract_json_from_response(&response);
        let (entity_info, relationships) = match &json {
            Some(v) => parse_entities_and_triples(v),
            None => (HashMap::new(), Vec::new()),
        };
        let entities = create_entities_from_parsed(&entity_info);
        let triples = create_triples_from_parsed(&relationships, &entities);

        let mut kg = KnowledgeGraph::new();
        for e in entities.values() {
            kg.add_entity(e.clone());
        }
        for t in &triples {
            kg.add_triple(t.clone());
        }

        let mut meta = HashMap::new();
        if let Some(v) = json {
            meta.insert("raw_json".into(), v);
        }
        let pr = ParsedResult {
            raw_response: response,
            entities_and_triples: Vec::new(),
            entities,
            relationships,
            triples,
            metadata: meta,
        };
        Some((pr, kg))
    }
}

#[async_trait::async_trait]
impl Extractor for TriplexExtractor {
    async fn extract(&self, text: &str) -> anyhow::Result<ExtractionResponse> {
        validate_input(text, self.config.min_segment_size, self.quiet)?;

        // Segment only when the text exceeds segment_size (matches Python).
        let segments: Vec<Segment> = if text.chars().count() > self.config.segment_size {
            segment(text, self.config.chunker, self.config.segment_size, self.config.overlap)
        } else {
            vec![Segment { content: text.to_string(), index: 0, start: 0, end: text.len() }]
        };

        let mut parsed_results = Vec::new();
        let mut graphs = Vec::new();
        for (i, seg) in segments.iter().enumerate() {
            if seg.content.chars().count() < self.config.min_segment_size && i > 0 {
                continue;
            }
            if let Some((pr, kg)) = self.extract_segment(&seg.content).await {
                parsed_results.push(pr);
                graphs.push(kg);
            }
        }

        // Honor merge_duplicates like SimpleExtractor, but fold segment graphs
        // with the collision-aware dedup from the start rather than running
        // `merge_all` first. Triplex JSON commonly reuses segment-local ids
        // (e.g. "e1") across segments; merge_all dedups only by id, so a second
        // segment's `e1` (a *different* entity) would be dropped before any
        // label dedup could run. merge_with_deduplication instead mints a fresh
        // id on an id-collision-with-different-label and remaps that segment's
        // triples onto it, so no entity is lost and relations stay attributed.
        let merged = if self.config.spec.merge_duplicates {
            graphs
                .into_iter()
                .fold(KnowledgeGraph::new(), |acc, g| merge_knowledge_graphs(acc, g, true))
        } else if graphs.is_empty() {
            KnowledgeGraph::new()
        } else {
            merge_all(graphs)
        };
        let mut resp = ExtractionResponse::new(merged);
        resp.parsed_results = parsed_results;
        resp.config = Some(self.config.clone());
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MockBackend;
    use crate::types::{EntityType, PredicateType};

    #[tokio::test]
    async fn parses_json_segment() {
        let json = r#"```json
        {"entities": {"e1": {"label": "OpenAI", "type": "organization"},
                       "e2": {"label": "GPT-4", "type": "technology"}},
         "relationships": [["e1", "developed_by", "e2"]]}
        ```"#;
        let backend = Arc::new(MockBackend::single(json));
        let ex = TriplexExtractor::new(backend);
        let out = ex.extract("OpenAI developed GPT-4.").await.unwrap();
        assert_eq!(out.num_entities(), 2);
        assert_eq!(out.num_triples(), 1);
        assert_eq!(out.knowledge_graph.entities["e1"].entity_type, EntityType::Organization);
        assert_eq!(out.knowledge_graph.triples[0].predicate.predicate_type, PredicateType::DevelopedBy);
    }

    #[tokio::test]
    async fn segmented_merge_preserves_colliding_local_ids() {
        use crate::types::ChunkStrategy;
        // Two segments both reuse local id "e1" for *different* entities.
        let seg1 = r#"{"entities": {"e1": {"label": "Alice", "type": "person"},
                                    "e2": {"label": "Acme", "type": "organization"}},
                       "relationships": [["e1", "works_for", "e2"]]}"#;
        let seg2 = r#"{"entities": {"e1": {"label": "Bob", "type": "person"},
                                    "e3": {"label": "Paris", "type": "city"}},
                       "relationships": [["e1", "located_in", "e3"]]}"#;
        let backend = Arc::new(MockBackend::new(vec![seg1.into(), seg2.into()]));

        let mut cfg = TriplexExtractor::default_config();
        cfg.segment_size = 60; // force segmentation
        cfg.overlap = 0;
        cfg.min_segment_size = 1;
        cfg.chunker = ChunkStrategy::Char;
        let ex = TriplexExtractor::with_config(backend, cfg);

        // 120 chars → exactly two 60-char Char chunks → seg1 then seg2.
        let text = "a".repeat(60) + &"b".repeat(60);
        let out = ex.extract(&text).await.unwrap();

        let labels: Vec<&str> = out.knowledge_graph.entities.values().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"Bob"), "Bob must survive the e1 collision: {labels:?}");
        assert_eq!(out.num_entities(), 4, "Alice/Acme/Bob/Paris all distinct: {labels:?}");
        assert_eq!(out.num_triples(), 2, "both relations must be kept and attributed");
    }

    #[tokio::test]
    async fn merge_duplicates_dedups_same_label_entities() {
        // Two entities with distinct ids but the same (case-insensitive) label.
        // With merge_duplicates (default) they must collapse to one.
        let json = r#"{"entities": {"e1": {"label": "OpenAI", "type": "organization"},
                                     "e2": {"label": "openai", "type": "organization"}},
                       "relationships": []}"#;
        let backend = Arc::new(MockBackend::single(json));
        let out = TriplexExtractor::new(backend).extract("OpenAI is OpenAI.").await.unwrap();
        assert_eq!(out.num_entities(), 1, "same-label entities must dedup when merge_duplicates");
    }
}
