//! JSON extraction from LLM responses.
//!
//! Split out of the former `parser` module (a TriplexExtractor remnant removed
//! in ADR-987 Phase A): `extract_json_from_response` was the only function in
//! it still reachable — the SchemaJson extractor and the community summarizer
//! both decode fenced LLM output through it.

use regex::Regex;
use std::sync::OnceLock;

/// Try to pull a JSON object out of an LLM response: ```json fence, ``` fence,
/// then the whole string.
pub fn extract_json_from_response(text: &str) -> Option<serde_json::Value> {
    static JSON_FENCE: OnceLock<Regex> = OnceLock::new();
    static ANY_FENCE: OnceLock<Regex> = OnceLock::new();

    let json_fence = JSON_FENCE.get_or_init(|| Regex::new(r"(?s)```json\s*(.*?)```").unwrap());
    if let Some(captures) = json_fence.captures(text) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(captures[1].trim()) {
            return Some(value);
        }
    }
    let any_fence = ANY_FENCE.get_or_init(|| Regex::new(r"(?s)```\s*(.*?)```").unwrap());
    if let Some(captures) = any_fence.captures(text) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(captures[1].trim()) {
            return Some(value);
        }
    }
    serde_json::from_str::<serde_json::Value>(text.trim()).ok()
}

#[cfg(test)]
#[path = "json_tests.rs"]
mod tests;
