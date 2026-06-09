//! Extraction strategies, ported from `graph/kg_extractor/{base,simple,triplex,youtu}.py`.
//!
//! All strategies implement the [`Extractor`] trait (the Rust analogue of the
//! Python `BaseExtractor`). Construction takes a backend ([`crate::backend::LlmBackend`])
//! and an [`ExtractionConfig`]; `extract` returns an [`ExtractionResponse`].

use crate::types::ExtractionResponse;
use async_trait::async_trait;

pub mod simple;
pub mod toolcall;
pub mod triplex;
pub mod youtu;

pub use simple::SimpleExtractor;
pub use toolcall::ToolCallExtractor;
pub use triplex::TriplexExtractor;
pub use youtu::YoutuExtractor;

/// How a seed schema constrains the schema-driven extractors (Youtu, ToolCall).
///
/// These are three of the four cells of the (schema present?) × (may add types?)
/// grid; the fourth — constrain to an *empty* schema — is meaningless and is made
/// unrepresentable by requiring a non-empty schema for `Fixed`/`Evolving`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaMode {
    /// No predefined schema; the model infers entity/relation types freely.
    Open,
    /// Closed: use only the types in the seeded schema. Requires a non-empty schema.
    Fixed,
    /// Seeded schema, but the model may propose new types (`new_schema_types`).
    /// Requires a non-empty schema (an empty seed is just `Open`).
    Evolving,
}

impl SchemaMode {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            SchemaMode::Open => "open",
            SchemaMode::Fixed => "fixed",
            SchemaMode::Evolving => "evolving",
        }
    }

    /// `Fixed`/`Evolving` extract against a seed schema; `Open` does not.
    pub(crate) fn needs_schema(&self) -> bool {
        matches!(self, SchemaMode::Fixed | SchemaMode::Evolving)
    }
}

/// Common extractor interface (analogue of Python `BaseExtractor`).
#[async_trait]
pub trait Extractor {
    /// Extract a knowledge graph from a single text document.
    async fn extract(&self, text: &str) -> anyhow::Result<ExtractionResponse>;
}

/// Validate input text against `min_segment_size`; warns (non-fatal) like the
/// Python `_validate_input`, errors only on empty input.
pub(crate) fn validate_input(text: &str, min_segment_size: usize, quiet: bool) -> anyhow::Result<()> {
    if text.trim().is_empty() {
        anyhow::bail!("No input text provided");
    }
    if text.chars().count() < min_segment_size && !quiet {
        eprintln!(
            "Warning: input too small ({} < {} chars)",
            text.chars().count(),
            min_segment_size
        );
    }
    Ok(())
}
