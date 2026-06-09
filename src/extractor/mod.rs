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
pub use youtu::{YoutuExtractor, YoutuMode};

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
