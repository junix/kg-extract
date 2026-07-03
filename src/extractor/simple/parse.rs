//! Delimiter-format output parsing for the SimpleExtractor.
//!
//! Parses the `(entity<|>…<|>…)` / `(relationship<|>…<|>…)` records the model
//! emits in response to [`super::prompts`] into typed entities + triples, with
//! the name-resolution, normalization and attribute-parsing helpers that
//! support it. Ported from `graph/kg_extractor/simple.py`.

use std::collections::HashMap;

use regex::Regex;

use crate::graph_build::{entity_id, should_swap_passive_by};
use crate::types::{
    Entity, EntityType, ExtractionConfig, ParsedResult, Predicate, PredicateType, Triple,
};

use super::TUPLE_DELIMITER;

/// Parse delimiter-formatted LLM output into entities + triples.
pub(crate) fn parse_output(output: &str, _config: &ExtractionConfig) -> ParsedResult {
    let items = split_to_items(output);
    let mut entities: HashMap<String, Entity> = HashMap::new();
    let mut entity_id_map: HashMap<String, String> = HashMap::new();
    let mut relationships: Vec<RelData> = Vec::new();

    for item in &items {
        let mut item = item.trim().to_string();
        if item.is_empty() {
            continue;
        }
        if item.starts_with('(') && !item.ends_with(')') {
            item.push(')');
        }
        // Classify by the record's leading token, not a substring scan of the
        // whole line: a relationship whose description mentions "entity" (e.g.
        // "these two entities are related") must not be misrouted to the entity
        // branch and silently dropped.
        let head = item
            .trim_start_matches('(')
            .split(TUPLE_DELIMITER)
            .next()
            .unwrap_or("")
            .to_lowercase();
        if head.contains("entity") && item.contains(TUPLE_DELIMITER) {
            if let Some(e) = parse_entity_item(&item) {
                entity_id_map.insert(normalize_key(&e.label), e.id.clone());
                entities.insert(e.id.clone(), e);
            }
        } else if head.contains("relationship") && item.contains(TUPLE_DELIMITER) {
            if let Some(r) = parse_relationship_item(&item, &entity_id_map) {
                relationships.push(r);
            }
        }
    }

    let mut triples = Vec::new();
    for rel in &relationships {
        let (Some(s), Some(o)) = (entities.get(&rel.source_id), entities.get(&rel.target_id))
        else {
            continue;
        };
        triples.push(build_triple_from_rel(rel, s, o));
    }

    let mut metadata = HashMap::new();
    metadata.insert("item_count".into(), serde_json::json!(items.len()));
    ParsedResult {
        raw_response: output.to_string(),
        entities_and_triples: Vec::new(),
        entities,
        relationships: Vec::new(),
        triples,
        metadata,
    }
}

/// Recover each entity record's **raw type token** (normalized name → raw type
/// string), which [`parse_output`] discards by mapping it into the
/// [`EntityType`] enum. Fixed-schema validation needs the literal token so a
/// domain-specific schema type (one outside the known `EntityType` vocabulary,
/// which `from_loose` would collapse to `Other`) can still be matched. Mirrors
/// [`parse_entity_item`]'s field positions exactly; keyed by [`normalize_key`]
/// so it lines up with an [`Entity`]'s lowercased label.
pub(crate) fn entity_type_tokens(output: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for item in split_to_items(output) {
        let mut item = item.trim().to_string();
        if item.is_empty() {
            continue;
        }
        if item.starts_with('(') && !item.ends_with(')') {
            item.push(')');
        }
        let inner = strip_parens(&item);
        let parts: Vec<&str> = inner.split(TUPLE_DELIMITER).collect();
        if parts.len() < 3 || !parts[0].to_lowercase().contains("entity") {
            continue;
        }
        let name = clean_entity_name(parts[1]);
        if name.is_empty() {
            continue;
        }
        let type_token = parts[2]
            .trim()
            .trim_matches(|c| "<> \"*#".contains(c))
            .to_string();
        out.insert(normalize_key(&name), type_token);
    }
    out
}

/// Parse a relation-only LLM response into triples, resolving each endpoint
/// against an existing entity table (by normalised label). Unlike [`parse_output`],
/// it does not require the entities to be re-declared in this response — the
/// rescue round emits relationships only. Edges with an unknown endpoint are
/// dropped (no dangling/hallucinated nodes).
pub(crate) fn parse_relations_against(
    output: &str,
    known: &HashMap<String, Entity>,
) -> Vec<Triple> {
    let id_map: HashMap<String, String> = known
        .values()
        .map(|e| (normalize_key(&e.label), e.id.clone()))
        .collect();

    let mut triples = Vec::new();
    for item in split_to_items(output) {
        let mut item = item.trim().to_string();
        if item.is_empty() {
            continue;
        }
        if item.starts_with('(') && !item.ends_with(')') {
            item.push(')');
        }
        let head = item
            .trim_start_matches('(')
            .split(TUPLE_DELIMITER)
            .next()
            .unwrap_or("")
            .to_lowercase();
        if !(head.contains("relationship") && item.contains(TUPLE_DELIMITER)) {
            continue;
        }
        let Some(rel) = parse_relationship_item(&item, &id_map) else {
            continue;
        };
        // Both endpoints must be entities we already know about.
        let (Some(s), Some(o)) = (known.get(&rel.source_id), known.get(&rel.target_id)) else {
            continue;
        };
        let mut t = build_triple_from_rel(&rel, s, o);
        t.metadata
            .insert("relation_gleaned".into(), serde_json::json!(true));
        triples.push(t);
    }
    triples
}

/// Materialize one parsed relationship into a [`Triple`], applying the
/// predicate-type inference, 50-char label clamp, passive-`*_BY` swap, and the
/// shared `description` / `source_name` / `target_name` metadata that every
/// simple-extractor edge carries. Shared by [`parse_output`] and
/// [`parse_relations_against`]; the rescue round stamps an extra
/// `relation_gleaned` flag on the returned triple.
pub(super) fn build_triple_from_rel(rel: &RelData, subject: &Entity, object: &Entity) -> Triple {
    let predicate_type = infer_predicate_type(&rel.predicate);
    let label: String = rel.predicate.chars().take(50).collect();
    let predicate = Predicate::with_label(predicate_type, label);
    let (subject, object) = if should_swap_passive_by(subject, &predicate, object) {
        (object.clone(), subject.clone())
    } else {
        (subject.clone(), object.clone())
    };
    let mut t = Triple::new(subject, predicate, object);
    t.confidence = Some(rel.strength);
    t.metadata
        .insert("description".into(), serde_json::json!(rel.description));
    t.metadata
        .insert("source_name".into(), serde_json::json!(rel.source_name));
    t.metadata
        .insert("target_name".into(), serde_json::json!(rel.target_name));
    t
}

/// One parsed relationship record, before it is materialized into a [`Triple`].
pub(super) struct RelData {
    pub(super) source_id: String,
    pub(super) target_id: String,
    pub(super) source_name: String,
    pub(super) target_name: String,
    pub(super) predicate: String,
    pub(super) description: String,
    pub(super) strength: f64,
}

pub(super) fn split_to_items(results: &str) -> Vec<String> {
    let entity_re = Regex::new(r#"\(\s*"?entity"?\s*<\|>"#).unwrap();
    let rel_re = Regex::new(r#"\(\s*"?relationship"?\s*<\|>"#).unwrap();
    let mut items = Vec::new();
    for part in results.split(super::RECORD_DELIMITER) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut starts: Vec<usize> = entity_re
            .find_iter(part)
            .chain(rel_re.find_iter(part))
            .map(|m| m.start())
            .collect();
        starts.sort_unstable();
        if starts.len() <= 1 {
            items.push(part.to_string());
        } else {
            for i in 0..starts.len() {
                let s = starts[i];
                let slice = if i == starts.len() - 1 {
                    &part[s..]
                } else {
                    &part[s..starts[i + 1]]
                };
                items.push(slice.trim().to_string());
            }
        }
    }
    items
}

pub(super) fn parse_entity_item(item: &str) -> Option<Entity> {
    let inner = strip_parens(item);
    let parts: Vec<&str> = inner.split(TUPLE_DELIMITER).collect();
    if parts.len() < 3 {
        return None;
    }
    let head = parts[0];
    if !head.to_lowercase().contains("entity") {
        return None;
    }
    let components = &parts[1..];
    let name = components.first().copied().unwrap_or("");
    let type_str = components.get(1).copied().unwrap_or("");
    let description = components.get(2).copied().unwrap_or("");
    let attrs = components.get(3).copied().unwrap_or("");

    let name = clean_entity_name(name);
    if name.is_empty() {
        return None;
    }
    let type_clean = type_str
        .trim()
        .trim_matches(|c| "<> \"*#".contains(c))
        .to_lowercase();
    let description = clean_description(description);
    let metadata = parse_attributes_string(attrs);

    let mut entity = Entity::new(entity_id(&name), &name, map_to_entity_type(&type_clean));
    entity.description = Some(description);
    if !metadata.is_empty() {
        entity.metadata = metadata;
    }
    Some(entity)
}

pub(super) fn parse_relationship_item(
    item: &str,
    id_map: &HashMap<String, String>,
) -> Option<RelData> {
    let inner = strip_parens(item);
    let parts: Vec<&str> = inner.split(TUPLE_DELIMITER).collect();
    if parts.len() < 4 {
        return None;
    }
    if !parts[0].to_lowercase().contains("relationship") {
        return None;
    }
    let components = &parts[1..];
    let source = clean_entity_name(components.first().copied().unwrap_or(""));
    let target = clean_entity_name(components.get(1).copied().unwrap_or(""));
    let predicate = clean_description(components.get(2).copied().unwrap_or(""));
    let description = clean_description(components.get(3).copied().unwrap_or(""));

    let strength = components
        .get(4)
        .map(|s| {
            let digits: String = s
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            digits
                .parse::<f64>()
                .map(|f| f.clamp(0.0, 1.0))
                .unwrap_or(0.8)
        })
        .unwrap_or(0.8);

    Some(RelData {
        source_id: resolve_entity_id(&source, id_map),
        target_id: resolve_entity_id(&target, id_map),
        source_name: source,
        target_name: target,
        predicate,
        description,
        strength,
    })
}

pub(super) fn strip_parens(item: &str) -> String {
    let item = item.trim();
    if item.starts_with('(') {
        if let Some(close) = item.rfind(')') {
            if close > 0 {
                return item[1..close].to_string();
            }
        }
    }
    item.to_string()
}

pub(super) fn normalize_key(name: &str) -> String {
    name.to_lowercase().trim().to_string()
}

fn resolve_entity_id(name: &str, id_map: &HashMap<String, String>) -> String {
    let key = normalize_key(name);
    id_map.get(&key).cloned().unwrap_or_else(|| entity_id(name))
}

fn clean_entity_name(name: &str) -> String {
    let name = name.trim_matches(|c| "<> \"'".contains(c));
    let no_quotes = Regex::new(r#"["'`]"#).unwrap().replace_all(name, "");
    let collapsed = Regex::new(r"\s+")
        .unwrap()
        .replace_all(no_quotes.trim(), " ");
    title_case(collapsed.trim())
}

fn clean_description(desc: &str) -> String {
    if desc.is_empty() {
        return "No description available".to_string();
    }
    let d = desc.trim_matches(|c| "<> \"'".contains(c));
    let d = Regex::new(r#"["'<>]"#).unwrap().replace_all(d, "");
    let d = Regex::new(r"\)\*\*|\*\*|\*\*##|\)")
        .unwrap()
        .replace_all(d.trim(), "");
    let d = Regex::new(r"\s+").unwrap().replace_all(d.trim(), " ");
    let out = d.trim().to_string();
    if out.is_empty() {
        "No description available".to_string()
    } else {
        out
    }
}

fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c
                    .to_uppercase()
                    .chain(chars.flat_map(|c| c.to_lowercase()))
                    .collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn map_to_entity_type(type_str: &str) -> EntityType {
    EntityType::from_loose(type_str)
}

fn infer_predicate_type(description: &str) -> PredicateType {
    let direct = PredicateType::from_loose(description);
    if direct != PredicateType::RelatedTo || description.trim().eq_ignore_ascii_case("related_to") {
        return direct;
    }

    let d = description.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| d.contains(w));
    if has(&["use", "utilize", "employ", "apply"]) {
        PredicateType::Uses
    } else if has(&["part of", "belong", "component", "member"]) {
        PredicateType::PartOf
    } else if has(&["located", "based in", "situated"]) {
        PredicateType::LocatedIn
    } else if has(&["work", "employed", "affiliated"]) {
        PredicateType::WorksFor
    } else if has(&["own", "possess", "has"]) {
        PredicateType::HasProperty
    } else {
        PredicateType::RelatedTo
    }
}

/// Parse `"key: value, key2: value2"` (or a JSON object) into metadata.
pub(crate) fn parse_attributes_string(s: &str) -> HashMap<String, serde_json::Value> {
    let s = s.trim();
    let mut out = HashMap::new();
    if s.is_empty() {
        return out;
    }
    if s.starts_with('{') && s.ends_with('}') {
        if let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(s) {
            return m.into_iter().collect();
        }
    }
    // Split on top-level commas.
    let mut pairs = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    for ch in s.chars().chain(std::iter::once(',')) {
        match ch {
            '(' | '{' | '[' => {
                depth += 1;
                current.push(ch);
            }
            ')' | '}' | ']' => {
                // Clamp at 0: an unmatched closing bracket must not drive depth
                // negative, which would suppress the depth==0 comma split (and the
                // trailing sentinel flush) and silently drop every attribute.
                depth = (depth - 1).max(0);
                current.push(ch);
            }
            ',' if depth == 0 => {
                let t = current.trim().to_string();
                if !t.is_empty() {
                    pairs.push(t);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    for pair in pairs {
        if let Some((k, v)) = pair.split_once(':') {
            let key = k.trim().trim_matches(|c| "\"'".contains(c)).to_string();
            let val = v.trim().trim_matches(|c| "\"'".contains(c));
            let value = if val.eq_ignore_ascii_case("true") {
                serde_json::json!(true)
            } else if val.eq_ignore_ascii_case("false") {
                serde_json::json!(false)
            } else if let Ok(i) = val.parse::<i64>() {
                serde_json::json!(i)
            } else if let Ok(f) = val.parse::<f64>() {
                serde_json::json!(f)
            } else {
                serde_json::json!(val)
            };
            out.insert(key, value);
        }
    }
    out
}
