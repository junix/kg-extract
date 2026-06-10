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
//! - [`SchemaJsonExtractor`](extractor::SchemaJsonExtractor) — schema-driven JSON
//!   extraction with three [`SchemaMode`](extractor::SchemaMode)s: open / fixed / evolving.
//! - [`ToolCallExtractor`](extractor::ToolCallExtractor) — extraction via LLM
//!   tool/function calling (typed `add_entity` / `add_relation` / … tools);
//!   structured by construction, so no output parsing.
//!
//! Text segmentation is delegated to the [`chonkie`] crate (see [`chunking`]);
//! the default strategy is recursive chunking. Completions come from either the
//! in-process `llms` crate ([`backend::LlmsBackend`], behind the `llms-backend`
//! feature) or an agent CLI driven over stream-json ([`backend::SdkAgentBackend`]:
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
pub mod citation;
pub mod extractor;
pub(crate) mod graph_build;
pub mod mcp;
pub mod merger;
pub mod parser;
pub mod template;
pub mod types;

// Re-exports for ergonomic top-level use.
pub use extractor::{
    Extractor, SchemaJsonExtractor, SchemaMode, SimpleExtractor, ToolCallExtractor,
};
pub use template::{render_prompt, TemplateCfg};
pub use types::{
    ChunkStrategy, Entity, EntityType, ExtractionConfig, ExtractionResponse, ExtractionSpec,
    KnowledgeGraph, Predicate, PredicateType, Schema, Triple,
};
