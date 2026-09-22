//! Export a [`KnowledgeGraph`](crate::types::KnowledgeGraph) as the JSON import
//! format understood by the sibling `graphdb-ladybug` CLI.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::types::KnowledgeGraph;

const FORMAT_VERSION: &str = "graphdb-ladybug.export.v1";
const ENTITY_TABLE: &str = "KgEntity";

/// Convert a knowledge graph into `graphdb-ladybug import --create-tables` JSON.
///
/// The layout is intentionally generic:
/// - all extracted entities land in one node table (`KgEntity`), with their KG
///   type stored as a property;
/// - each predicate gets its own relationship table, so relation queries can
///   target the concrete relationship type while still preserving the original
///   predicate/label/metadata as properties.
pub fn to_ladybug_import_json(kg: &KnowledgeGraph) -> Value {
    let rel_types: BTreeSet<String> = kg
        .triples
        .iter()
        .map(|t| sanitize_identifier(&t.predicate.output_type()))
        .collect();

    let mut schema = vec![format!(
        "CREATE NODE TABLE {ENTITY_TABLE}(id STRING, label STRING, type STRING, description STRING, confidence DOUBLE, metadata STRING, PRIMARY KEY(id));"
    )];
    for rel_type in &rel_types {
        schema.push(format!(
            "CREATE REL TABLE {rel_type}(FROM {ENTITY_TABLE} TO {ENTITY_TABLE}, predicate STRING, label STRING, confidence DOUBLE, metadata STRING);"
        ));
    }

    let nodes: Vec<Value> = kg
        .entities
        .iter()
        .map(|(_, entity)| {
            json!({
                "_table": ENTITY_TABLE,
                "id": entity.id,
                "label": entity.label,
                "type": entity.output_type(),
                "description": entity.description,
                "confidence": entity.confidence,
                "metadata": entity_metadata_string(entity),
            })
        })
        .collect();

    let relationships: Vec<Value> = kg
        .triples
        .iter()
        .map(|triple| {
            let rel_type = sanitize_identifier(&triple.predicate.output_type());
            json!({
                "_type": rel_type,
                "_from": triple.subject.id,
                "_to": triple.object.id,
                "_from_table": ENTITY_TABLE,
                "_to_table": ENTITY_TABLE,
                "predicate": triple.predicate.output_type(),
                "label": triple.predicate.display_label(),
                "confidence": triple.confidence.or(triple.predicate.confidence),
                "metadata": relationship_metadata_string(triple),
            })
        })
        .collect();

    json!({
        "format_version": FORMAT_VERSION,
        "schema": schema,
        "nodes": nodes,
        "relationships": relationships,
    })
}

fn entity_metadata_string(entity: &crate::types::Entity) -> String {
    let mut merged: BTreeMap<String, Value> = entity
        .metadata
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    merged.insert("normalized_type".into(), json!(entity.entity_type.value()));
    serde_json::to_string(&merged).unwrap_or_else(|_| "{}".to_string())
}

fn relationship_metadata_string(triple: &crate::types::Triple) -> String {
    let mut merged = BTreeMap::new();
    merged.insert("triple", json!(triple.metadata));
    let mut predicate_metadata = triple.predicate.metadata.clone();
    predicate_metadata.insert(
        "normalized_type".into(),
        json!(triple.predicate.predicate_type.value()),
    );
    merged.insert("predicate", json!(predicate_metadata));
    serde_json::to_string(&merged).unwrap_or_else(|_| "{}".to_string())
}

fn sanitize_identifier(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("RELATED_TO");
    }
    if !out
        .as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
    {
        out.insert(0, '_');
    }
    out
}

#[cfg(test)]
#[path = "ladybug_export_tests.rs"]
mod tests;

