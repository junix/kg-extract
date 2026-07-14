//! Parse LLM responses into entities and relationships.
//!
//! Ported from `graph/kg_extractor/parser.py`. Used by the SchemaJson
//! JSON-style extractor. The Simple extractor has its own delimiter parser.

mod build;
mod decode;

use crate::types::ParsedResult;
use std::collections::HashMap;

pub use build::{create_entities_from_parsed, create_triples_from_parsed};
pub use decode::{extract_json_from_response, parse_entities_and_triples, EntityInfo, RelTuple};

/// Parse an LLM response into a [`ParsedResult`] (entities + relationship tuples).
pub fn parse_llm_response(response_text: &str) -> ParsedResult {
    let json_data = match extract_json_from_response(response_text) {
        Some(v) => v,
        None => {
            let mut meta = HashMap::new();
            meta.insert(
                "parse_error".into(),
                serde_json::json!("No JSON found in response"),
            );
            return ParsedResult {
                raw_response: response_text.to_string(),
                metadata: meta,
                ..Default::default()
            };
        }
    };

    let (entity_info, relationships) = parse_entities_and_triples(&json_data);
    let mut meta = HashMap::new();
    meta.insert("raw_json".into(), json_data.clone());

    let entities_and_triples = json_data
        .get("entities_and_triples")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let entities = create_entities_from_parsed(&entity_info);

    let mut parsed = ParsedResult {
        raw_response: response_text.to_string(),
        entities_and_triples,
        entities,
        relationships,
        triples: Vec::new(),
        metadata: meta,
    };
    parsed.metadata.insert(
        "entities_info".into(),
        serde_json::to_value(&entity_info).unwrap_or(serde_json::Value::Null),
    );
    parsed
}

#[cfg(test)]
mod tests;
