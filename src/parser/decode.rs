use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// Raw entity info as parsed from JSON, before mapping to a domain entity.
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

/// A `(source_id, relation, target_id)` relationship tuple.
pub type RelTuple = (String, String, String);

/// Extract `(entities_info, relationships)` from structured JSON.
pub fn parse_entities_and_triples(
    json_data: &serde_json::Value,
) -> (HashMap<String, EntityInfo>, Vec<RelTuple>) {
    let mut entities: HashMap<String, EntityInfo> = HashMap::new();
    let mut relationships: Vec<RelTuple> = Vec::new();

    if let Some(entities_value) = json_data.get("entities") {
        if let Some(object) = entities_value.as_object() {
            for (id, info) in object {
                if let Some(info_object) = info.as_object() {
                    entities.insert(
                        id.clone(),
                        EntityInfo {
                            label: info_object
                                .get("label")
                                .and_then(|value| value.as_str())
                                .unwrap_or(id)
                                .to_string(),
                            r#type: info_object
                                .get("type")
                                .and_then(|value| value.as_str())
                                .map(String::from),
                            description: info_object
                                .get("description")
                                .and_then(|value| value.as_str())
                                .map(String::from),
                            attributes: info_object
                                .get("attributes")
                                .and_then(|value| value.as_object())
                                .map(|attributes| {
                                    attributes
                                        .iter()
                                        .map(|(key, value)| (key.clone(), value.clone()))
                                        .collect()
                                })
                                .unwrap_or_default(),
                        },
                    );
                } else {
                    entities.insert(
                        id.clone(),
                        EntityInfo {
                            label: value_to_string(info),
                            ..Default::default()
                        },
                    );
                }
            }
        } else if let Some(array) = entities_value.as_array() {
            let reserved_ids: HashSet<&str> = array
                .iter()
                .filter_map(|entity| entity.get("id").and_then(|id| id.as_str()))
                .collect();
            let mut next_generated_id = 0usize;
            for entity in array {
                if let Some(object) = entity.as_object() {
                    let id = match object.get("id").and_then(|value| value.as_str()) {
                        Some(id) => id.to_string(),
                        None => loop {
                            let candidate = format!("entity_{next_generated_id}");
                            next_generated_id += 1;
                            if !reserved_ids.contains(candidate.as_str())
                                && !entities.contains_key(&candidate)
                            {
                                break candidate;
                            }
                        },
                    };
                    let label = object
                        .get("label")
                        .or_else(|| object.get("name"))
                        .and_then(|value| value.as_str())
                        .unwrap_or(&id)
                        .to_string();
                    entities.insert(
                        id,
                        EntityInfo {
                            label,
                            r#type: object
                                .get("type")
                                .and_then(|value| value.as_str())
                                .map(String::from),
                            description: object
                                .get("description")
                                .and_then(|value| value.as_str())
                                .map(String::from),
                            attributes: object
                                .get("attributes")
                                .and_then(|value| value.as_object())
                                .map(|attributes| {
                                    attributes
                                        .iter()
                                        .map(|(key, value)| (key.clone(), value.clone()))
                                        .collect()
                                })
                                .unwrap_or_default(),
                        },
                    );
                }
            }
        }
    }

    if let Some(relations) = json_data
        .get("relationships")
        .and_then(|value| value.as_array())
    {
        for relation in relations {
            if let Some(array) = relation.as_array() {
                if array.len() >= 3 {
                    relationships.push((
                        value_to_string(&array[0]),
                        value_to_string(&array[1]),
                        value_to_string(&array[2]),
                    ));
                }
            } else if let Some(object) = relation.as_object() {
                let source = object.get("source").or_else(|| object.get("subject"));
                let target = object.get("target").or_else(|| object.get("object"));
                let predicate = object.get("predicate").or_else(|| object.get("relation"));
                if let (Some(source), Some(target), Some(predicate)) = (source, target, predicate) {
                    relationships.push((
                        value_to_string(source),
                        value_to_string(predicate),
                        value_to_string(target),
                    ));
                }
            }
        }
    }

    if entities.is_empty() {
        if let Some(items) = json_data
            .get("entities_and_triples")
            .and_then(|value| value.as_array())
        {
            let id_pattern = Regex::new(r"\[\d+\]").unwrap();
            for item in items {
                let Some(item) = item.as_str() else { continue };
                let markers: Vec<&str> = id_pattern
                    .find_iter(item)
                    .map(|marker| marker.as_str())
                    .collect();
                match markers.len() {
                    1 => {
                        if let Some((id, label)) = item.split_once(", ") {
                            entities.insert(
                                id.trim().to_string(),
                                EntityInfo {
                                    label: label.trim().to_string(),
                                    ..Default::default()
                                },
                            );
                        }
                    }
                    2 => {
                        let between: Vec<String> = id_pattern
                            .split(item)
                            .map(|part| part.trim().to_string())
                            .filter(|part| !part.is_empty())
                            .collect();
                        if between.len() == 1 {
                            relationships.push((
                                markers[0].to_string(),
                                between[0].clone(),
                                markers[1].to_string(),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    (entities, relationships)
}

fn value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(string) => string.clone(),
        other => other.to_string(),
    }
}
