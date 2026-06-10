//! Extraction strategies, ported from `graph/kg_extractor/{base,simple,youtu}.py`.
//!
//! All strategies implement the [`Extractor`] trait (the Rust analogue of the
//! Python `BaseExtractor`). Construction takes a backend ([`crate::backend::LlmBackend`])
//! and an [`ExtractionConfig`]; `extract` returns an [`ExtractionResponse`].
//!
//! # Two orthogonal axes
//!
//! The strategies vary along two independent dimensions:
//!
//! - **Mechanism** — *how* the model is driven:
//!   - **Prompt → parse**: the model emits text we parse. [`SimpleExtractor`]
//!     (delimiter format + gleaning recall), [`SchemaJsonExtractor`]
//!     (schema-guided JSON).
//!   - **Tool call → structured**: the model calls typed tools; no parsing.
//!     [`ToolCallExtractor`]. The MCP server ([`crate::mcp`]) exposes the same
//!     tool/graph-building core to an *external* agent.
//! - **Schema mode** — *how the schema constrains extraction*: [`SchemaMode`]
//!   (`Open` / `Fixed` / `Evolving`). Orthogonal to mechanism; first-class on
//!   SchemaJson and ToolCall today.
//!
//! Graph construction shared across mechanisms (the id scheme, name-based
//! relationship resolution, dangling-endpoint dropping) lives in
//! [`crate::graph_build`].

use crate::chunking::Segment;
use crate::types::ExtractionResponse;
use async_trait::async_trait;

pub mod agentic;
pub mod schema_json;
pub mod simple;
pub mod toolcall;

pub use agentic::AgenticExtractor;
pub use schema_json::SchemaJsonExtractor;
pub use simple::SimpleExtractor;
pub use toolcall::ToolCallExtractor;

// `SchemaMode` is part of the declarative spec; it lives in `types::spec` so
// `ExtractionSpec` can hold it without a types→extractor cycle. Re-exported here
// since it's the schema-driven extractors' primary knob.
pub use crate::types::SchemaMode;

/// Common extractor interface (analogue of Python `BaseExtractor`).
#[async_trait]
pub trait Extractor {
    /// Extract a knowledge graph from a single text document.
    async fn extract(&self, text: &str) -> anyhow::Result<ExtractionResponse>;

    /// Extract from pre-chunked input (e.g. chonkie chunks parsed by
    /// [`crate::chunking::parse_prechunked`]). The chunking engines
    /// ([`SimpleExtractor`], [`AgenticExtractor`]) override this to consume the
    /// chunks **as-is** instead of re-chunking; this default — for the
    /// single-shot engines that never chunk ([`SchemaJsonExtractor`],
    /// [`ToolCallExtractor`]) — joins the chunk texts and extracts over them
    /// exactly as it would over plain-text input.
    async fn extract_prechunked(&self, chunks: &[Segment]) -> anyhow::Result<ExtractionResponse> {
        let text = chunks
            .iter()
            .map(|s| s.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        self.extract(&text).await
    }
}

/// Validate input text against `min_segment_size`; warns (non-fatal) like the
/// Python `_validate_input`, errors only on empty input.
pub(crate) fn validate_input(
    text: &str,
    min_segment_size: usize,
    quiet: bool,
) -> anyhow::Result<()> {
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
