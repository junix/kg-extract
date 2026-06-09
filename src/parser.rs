//! Parse LLM responses into entities and relationships.
//!
//! Ported from `graph/kg_extractor/parser.py`. Used by the Triplex and Youtu
//! JSON-style extractors. The Simple extractor has its own delimiter parser.

use crate::types::{Entity, EntityType, ParsedResult, Predicate, PredicateType, Triple};
use regex::Regex;
use std::collections::HashMap;

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
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();

    // Build the Entity objects up front so `ParsedResult.entities` is actually
    // populated (it was previously always empty). The raw info dict is still
    // kept in metadata for callers that want the pre-mapping form.
    let entities = create_entities_from_parsed(&entity_info);

    let mut pr = ParsedResult {
        raw_response: response_text.to_string(),
        entities_and_triples,
        entities,
        relationships,
        triples: Vec::new(),
        metadata: meta,
    };
    pr.metadata.insert(
        "entities_info".into(),
        serde_json::to_value(&entity_info).unwrap_or(serde_json::Value::Null),
    );
    pr
}

/// Raw entity info as parsed from JSON, before mapping to [`Entity`].
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EntityInfo {
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub attributes: HashMap<String, serde_json::Value>,
}

/// Try to pull a JSON object out of an LLM response: ```json fence, ``` fence,
/// then the whole string.
pub fn extract_json_from_response(text: &str) -> Option<serde_json::Value> {
    let json_fence = Regex::new(r"(?s)```json\s*(.*?)```").unwrap();
    if let Some(c) = json_fence.captures(text) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(c[1].trim()) {
            return Some(v);
        }
    }
    let any_fence = Regex::new(r"(?s)```\s*(.*?)```").unwrap();
    if let Some(c) = any_fence.captures(text) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(c[1].trim()) {
            return Some(v);
        }
    }
    serde_json::from_str::<serde_json::Value>(text.trim()).ok()
}

/// A `(source_id, relation, target_id)` relationship tuple.
pub type RelTuple = (String, String, String);

/// Extract `(entities_info, relationships)` from structured JSON.
pub fn parse_entities_and_triples(
    json_data: &serde_json::Value,
) -> (HashMap<String, EntityInfo>, Vec<RelTuple>) {
    let mut entities: HashMap<String, EntityInfo> = HashMap::new();
    let mut relationships: Vec<(String, String, String)> = Vec::new();

    if let Some(ents) = json_data.get("entities") {
        if let Some(obj) = ents.as_object() {
            for (id, info) in obj {
                if let Some(info_obj) = info.as_object() {
                    entities.insert(
                        id.clone(),
                        EntityInfo {
                            label: info_obj
                                .get("label")
                                .and_then(|v| v.as_str())
                                .unwrap_or(id)
                                .to_string(),
                            r#type: info_obj.get("type").and_then(|v| v.as_str()).map(String::from),
                            description: info_obj
                                .get("description")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            attributes: info_obj
                                .get("attributes")
                                .and_then(|v| v.as_object())
                                .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                                .unwrap_or_default(),
                        },
                    );
                } else {
                    entities.insert(
                        id.clone(),
                        EntityInfo { label: value_to_string(info), ..Default::default() },
                    );
                }
            }
        } else if let Some(arr) = ents.as_array() {
            for ent in arr {
                if let Some(o) = ent.as_object() {
                    let id = o
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or_else(|| format!("entity_{}", entities.len()));
                    let label = o
                        .get("label")
                        .or_else(|| o.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&id)
                        .to_string();
                    entities.insert(
                        id,
                        EntityInfo {
                            label,
                            r#type: o.get("type").and_then(|v| v.as_str()).map(String::from),
                            description: o.get("description").and_then(|v| v.as_str()).map(String::from),
                            attributes: o
                                .get("attributes")
                                .and_then(|v| v.as_object())
                                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                                .unwrap_or_default(),
                        },
                    );
                }
            }
        }
    }

    if let Some(rels) = json_data.get("relationships").and_then(|v| v.as_array()) {
        for rel in rels {
            if let Some(arr) = rel.as_array() {
                if arr.len() >= 3 {
                    relationships.push((
                        value_to_string(&arr[0]),
                        value_to_string(&arr[1]),
                        value_to_string(&arr[2]),
                    ));
                }
            } else if let Some(o) = rel.as_object() {
                let source = o.get("source").or_else(|| o.get("subject"));
                let target = o.get("target").or_else(|| o.get("object"));
                let predicate = o.get("predicate").or_else(|| o.get("relation"));
                if let (Some(s), Some(t), Some(p)) = (source, target, predicate) {
                    relationships.push((value_to_string(s), value_to_string(p), value_to_string(t)));
                }
            }
        }
    }

    // Legacy fallback: the `entities_and_triples` list with `[N]` ID markers,
    // emitted by Triplex-style models. One `[N]` → entity; two → relationship.
    if entities.is_empty() {
        if let Some(items) = json_data.get("entities_and_triples").and_then(|v| v.as_array()) {
            let id_re = Regex::new(r"\[\d+\]").unwrap();
            for item in items {
                let Some(item) = item.as_str() else { continue };
                let markers: Vec<&str> = id_re.find_iter(item).map(|m| m.as_str()).collect();
                match markers.len() {
                    1 => {
                        if let Some((id, label)) = item.split_once(", ") {
                            entities.insert(
                                id.trim().to_string(),
                                EntityInfo { label: label.trim().to_string(), ..Default::default() },
                            );
                        }
                    }
                    2 => {
                        // `[src] relation [tgt]`. Rust's split drops the markers,
                        // so the single non-empty remainder is the relation text
                        // (Python kept markers and required exactly [src],rel,[tgt]).
                        let between: Vec<String> = id_re
                            .split(item)
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        if between.len() == 1 {
                            relationships.push((markers[0].to_string(), between[0].clone(), markers[1].to_string()));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    (entities, relationships)
}

/// Build [`Entity`] objects from parsed entity-info (ported from
/// `create_entities_from_parsed`, including the heuristic type fallback).
pub fn create_entities_from_parsed(entities: &HashMap<String, EntityInfo>) -> HashMap<String, Entity> {
    let mut result = HashMap::new();
    for (id, data) in entities {
        let label = if data.label.is_empty() { id.clone() } else { data.label.clone() };

        let entity_type = if let Some(type_str) = &data.r#type {
            EntityType::from_loose(type_str)
        } else {
            heuristic_type(&label)
        };

        let mut entity = Entity::new(id.clone(), label, entity_type);
        entity.description = data.description.clone();
        if !data.attributes.is_empty() {
            entity.metadata = data.attributes.clone();
        }
        result.insert(id.clone(), entity);
    }
    result
}

fn heuristic_type(label: &str) -> EntityType {
    let l = label.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| l.contains(w));
    if has(&["person", "people", "human"]) {
        EntityType::Person
    } else if has(&["company", "corporation", "inc", "ltd"]) {
        EntityType::Company
    } else if has(&["organization", "institute", "agency"]) {
        EntityType::Organization
    } else if has(&["city", "town", "village"]) {
        EntityType::City
    } else if has(&["country", "nation", "state"]) {
        EntityType::Country
    } else if has(&["technology", "software", "system"]) {
        EntityType::Technology
    } else {
        EntityType::PhysicalObject
    }
}

/// Build [`Triple`] objects from relationship tuples (ported from
/// `create_triples_from_parsed`). Skips relations whose endpoints are unknown.
pub fn create_triples_from_parsed(
    relationships: &[(String, String, String)],
    entities: &HashMap<String, Entity>,
) -> Vec<Triple> {
    let mut triples = Vec::new();
    for (source_id, relation, target_id) in relationships {
        let (Some(s), Some(o)) = (entities.get(source_id), entities.get(target_id)) else {
            continue;
        };
        let predicate_type = PredicateType::from_loose(relation);
        let predicate = Predicate::with_label(predicate_type, relation.clone());
        triples.push(Triple::new(s.clone(), predicate, o.clone()));
    }
    triples
}

fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fenced_json() {
        let resp = "```json\n{\"entities\": {\"e1\": {\"label\": \"GPT-4\", \"type\": \"technology\"}}, \"relationships\": [[\"e1\", \"uses\", \"e1\"]]}\n```";
        let pr = parse_llm_response(resp);
        let info: HashMap<String, EntityInfo> =
            serde_json::from_value(pr.metadata["entities_info"].clone()).unwrap();
        let entities = create_entities_from_parsed(&info);
        assert_eq!(entities["e1"].entity_type, EntityType::Technology);
        let triples = create_triples_from_parsed(&pr.relationships, &entities);
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].predicate.predicate_type, PredicateType::Uses);
    }

    #[test]
    fn parse_llm_response_populates_entities() {
        // `ParsedResult.entities` must contain built Entity objects, not be empty.
        let resp = r#"```json
        {"entities": {"e1": {"label": "GPT-4", "type": "technology"}}, "relationships": []}
        ```"#;
        let pr = parse_llm_response(resp);
        assert!(!pr.entities.is_empty(), "entities must be populated");
        assert_eq!(pr.entities["e1"].label, "GPT-4");
        assert_eq!(pr.entities["e1"].entity_type, EntityType::Technology);
    }

    #[test]
    fn parse_legacy_entities_and_triples() {
        let json: serde_json::Value = serde_json::json!({
            "entities_and_triples": [
                "[1], OpenAI",
                "[2], GPT-4",
                "[1] developed_by [2]"
            ]
        });
        let (entities, rels) = parse_entities_and_triples(&json);
        assert_eq!(entities.len(), 2);
        assert_eq!(entities["[1]"].label, "OpenAI");
        assert_eq!(rels, vec![("[1]".to_string(), "developed_by".to_string(), "[2]".to_string())]);
        let built = create_entities_from_parsed(&entities);
        let triples = create_triples_from_parsed(&rels, &built);
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].predicate.predicate_type, PredicateType::DevelopedBy);
    }
}
