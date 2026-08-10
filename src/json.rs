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
mod tests {
    use super::extract_json_from_response;

    // -----------------------------------------------------------------------
    // The three-tier fallback chain (```json fence -> ``` fence -> whole
    // string) and its fall-through semantics.
    // -----------------------------------------------------------------------

    #[test]
    fn extract_json_prefers_json_fence_over_trailing_bare_json() {
        // Both the ```json fence and a trailing bare object parse; the fence wins.
        let text = "```json\n{\"a\": 1}\n```\n{\"b\": 2}";
        let v = extract_json_from_response(text).expect("json fence must parse");
        assert_eq!(v, serde_json::json!({"a": 1}));
    }

    #[test]
    fn extract_json_falls_back_to_plain_fence_without_json_tag() {
        let text = "```\n{\"x\": 5}\n```";
        let v = extract_json_from_response(text).expect("plain fence must parse");
        assert_eq!(v, serde_json::json!({"x": 5}));
    }

    #[test]
    fn extract_json_falls_back_to_whole_string_without_any_fence() {
        let v = extract_json_from_response("{\"k\": \"v\"}").expect("bare string must parse");
        assert_eq!(v, serde_json::json!({"k": "v"}));
    }

    #[test]
    fn extract_json_malformed_in_fence_does_not_panic_and_yields_none() {
        // The ```json fence captures malformed JSON; the plain-fence fallback then
        // re-captures "json\n{not valid}\n" (also malformed); the whole string still
        // carries the fences. Every tier fails -> None, proving the fall-through is
        // exhaustive rather than short-circuiting on the first (failed) capture.
        let text = "```json\n{not valid}\n```";
        assert!(extract_json_from_response(text).is_none());
    }

    #[test]
    fn extract_json_returns_none_for_garbage() {
        assert!(extract_json_from_response("the model rambled, no JSON").is_none());
        assert!(extract_json_from_response("").is_none());
    }
}
