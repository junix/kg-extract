//! Extraction config, request, parsed-result and response types
//! (ported from `graph/_types/extraction.py`).

use super::entity::{default_entity_types, Entity};
use super::graph::{KnowledgeGraph, Triple};
use super::predicate::default_predicates;
use super::schema::Schema;
use super::spec::{ExtractionSpec, SchemaMode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Which chunker chonkie should use to segment input before extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChunkStrategy {
    /// Character sliding window — exact parity with the Python `segment_chunks`.
    Char,
    /// chonkie recursive splitting (paragraph → sentence → … ); better boundaries.
    #[default]
    Recursive,
    /// chonkie token-based splitting (real token counts).
    Token,
}

/// Configuration for knowledge graph extraction.
///
/// Two layers: the declarative [`ExtractionSpec`] (the *what* — schema, mode,
/// dedup policy) and the execution params (the *how* — model, chunking,
/// segmentation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionConfig {
    /// The declarative extraction contract (schema / mode / dedup).
    #[serde(default)]
    pub spec: ExtractionSpec,
    pub segment_size: usize,
    pub overlap: usize,
    pub model_name: String,
    pub min_segment_size: usize,
    #[serde(default)]
    pub chunker: ChunkStrategy,
    /// Max segments extracted concurrently. LLM calls are I/O-bound, so this is
    /// cooperative concurrency (not CPU parallelism). `1` = sequential.
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
}

fn default_max_concurrency() -> usize {
    8
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        // Mirrors Python: empty schema → seed with all default entity/predicate types.
        let schema = Schema::new(
            default_entity_types().iter().map(|e| e.value()).collect(),
            default_predicates().iter().map(|p| p.value()).collect(),
            Vec::new(),
        );
        ExtractionConfig {
            spec: ExtractionSpec { schema, ..Default::default() },
            segment_size: 3000,
            overlap: 200,
            model_name: "qwen-max".to_string(),
            min_segment_size: 100,
            chunker: ChunkStrategy::default(),
            max_concurrency: default_max_concurrency(),
        }
    }
}

impl ExtractionConfig {
    /// Build from an explicit schema (the empty schema is left as-is rather than
    /// seeded with defaults — mirrors `ExtractionConfig.from_schema`).
    pub fn from_schema(schema: Schema) -> Self {
        ExtractionConfig { spec: ExtractionSpec { schema, ..Default::default() }, ..Default::default() }
    }

    /// Build from legacy entity/predicate string lists (`from_legacy`).
    pub fn from_legacy(entity_types: Vec<String>, predicates: Vec<String>) -> Self {
        ExtractionConfig::from_schema(Schema::new(entity_types, predicates, Vec::new()))
    }

    /// Build with the given declarative spec and default execution params.
    pub fn from_spec(spec: ExtractionSpec) -> Self {
        ExtractionConfig { spec, ..Default::default() }
    }

    pub fn entity_types_list(&self) -> &[String] {
        &self.spec.schema.nodes
    }
    pub fn predicates_list(&self) -> &[String] {
        &self.spec.schema.relations
    }
    pub fn attributes_list(&self) -> &[String] {
        &self.spec.schema.attributes
    }
}

/// A request to extract from a single combined text.
#[derive(Debug, Clone)]
pub struct ExtractionRequest {
    pub text: String,
    pub config: ExtractionConfig,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl ExtractionRequest {
    pub fn new(text: impl Into<String>, config: ExtractionConfig) -> Self {
        ExtractionRequest { text: text.into(), config, metadata: HashMap::new() }
    }
}

/// Intermediate parse result for one LLM response / segment.
#[derive(Debug, Clone, Default)]
pub struct ParsedResult {
    pub raw_response: String,
    /// Legacy `entities_and_triples` strings (JSON ID-marker path).
    pub entities_and_triples: Vec<String>,
    /// Parsed entities keyed by id.
    pub entities: HashMap<String, Entity>,
    /// `(source_id, relation, target_id)` relationship tuples.
    pub relationships: Vec<(String, String, String)>,
    /// Fully-built triples (Simple extractor populates these directly).
    pub triples: Vec<Triple>,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Final response from extraction.
#[derive(Debug, Clone)]
pub struct ExtractionResponse {
    pub knowledge_graph: KnowledgeGraph,
    pub parsed_results: Vec<ParsedResult>,
    pub config: Option<ExtractionConfig>,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl ExtractionResponse {
    pub fn new(knowledge_graph: KnowledgeGraph) -> Self {
        ExtractionResponse {
            knowledge_graph,
            parsed_results: Vec::new(),
            config: None,
            metadata: HashMap::new(),
        }
    }

    pub fn num_entities(&self) -> usize {
        self.knowledge_graph.entities.len()
    }
    pub fn num_triples(&self) -> usize {
        self.knowledge_graph.triples.len()
    }
    pub fn get_mermaid_code(&self) -> String {
        self.knowledge_graph.to_mermaid()
    }
    pub fn get_stats(&self) -> serde_json::Value {
        let mut stats = self.knowledge_graph.stats();
        if let Some(obj) = stats.as_object_mut() {
            obj.insert(
                "num_segments_processed".into(),
                serde_json::json!(self.parsed_results.len()),
            );
        }
        stats
    }

    pub fn to_dict(&self) -> serde_json::Value {
        serde_json::json!({
            "knowledge_graph": self.knowledge_graph.to_dict(),
            "metadata": self.metadata,
        })
    }
}
