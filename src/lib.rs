//! # kg-extract
//!
//! Multi-strategy knowledge-graph extraction, ported from the Python
//! `graph.kg_extractor` module.
//!
//! Three extraction strategies share a common [`Extractor`](extractor::Extractor)
//! trait and pluggable completion [`backend`]s:
//!
//! - [`SimpleExtractor`](extractor::SimpleExtractor) — general LLM chat with
//!   GraphRAG-style delimiter prompting and multi-gleaning (high recall).
//! - [`TriplexExtractor`](extractor::TriplexExtractor) — NER + triple extraction
//!   via a Triplex-style model (default `sciphi/triplex` on Ollama), with
//!   segmentation for large inputs.
//! - [`YoutuExtractor`](extractor::YoutuExtractor) — schema-driven extraction
//!   with optional agent mode (schema evolution) and community detection.
//!
//! Text segmentation is delegated to the [`chonkie`] crate (see [`chunking`]);
//! the default strategy is recursive chunking. Completions come from either the
//! in-process `llms` crate ([`backend::LlmsBackend`], behind the `llms-backend`
//! feature) or a subprocess agent CLI ([`backend::AgentCliBackend`]:
//! `minimaxcc` / `glmcc` / `mimocc`).
//!
//! ```no_run
//! use std::sync::Arc;
//! use kg_extract::backend::MockBackend;
//! use kg_extract::extractor::{Extractor, SimpleExtractor};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let backend = Arc::new(MockBackend::single("(entity<|>OpenAI<|>organization<|>An AI lab.<|>)##"));
//! let extractor = SimpleExtractor::new(backend);
//! let response = extractor.extract("OpenAI built GPT-4.").await?;
//! println!("{}", response.get_mermaid_code());
//! # Ok(()) }
//! ```

pub mod backend;
pub mod chunking;
pub mod extractor;
pub mod merger;
pub mod parser;
pub mod types;

// Re-exports for ergonomic top-level use.
pub use extractor::{Extractor, SimpleExtractor, TriplexExtractor, YoutuExtractor, YoutuMode};
pub use types::{
    ChunkStrategy, Entity, EntityType, ExtractionConfig, ExtractionResponse, KnowledgeGraph,
    Predicate, PredicateType, Schema, Triple,
};
