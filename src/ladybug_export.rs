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
        .map(|t| sanitize_identifier(&t.predicate.predicate_type.value()))
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
                "type": entity.entity_type.value(),
                "description": entity.description,
                "confidence": entity.confidence,
                "metadata": metadata_string(&entity.metadata),
            })
        })
        .collect();

    let relationships: Vec<Value> = kg
        .triples
        .iter()
        .map(|triple| {
            let rel_type = sanitize_identifier(&triple.predicate.predicate_type.value());
            json!({
                "_type": rel_type,
                "_from": triple.subject.id,
                "_to": triple.object.id,
                "_from_table": ENTITY_TABLE,
                "_to_table": ENTITY_TABLE,
                "predicate": triple.predicate.predicate_type.value(),
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

fn metadata_string(metadata: &std::collections::HashMap<String, Value>) -> String {
    serde_json::to_string(metadata).unwrap_or_else(|_| "{}".to_string())
}

fn relationship_metadata_string(triple: &crate::types::Triple) -> String {
    let mut merged = BTreeMap::new();
    merged.insert("triple", json!(triple.metadata));
    merged.insert("predicate", json!(triple.predicate.metadata));
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
mod tests {
    use crate::types::{Entity, EntityType, Predicate, PredicateType, Triple};

    use super::to_ladybug_import_json;

    #[test]
    fn exports_generic_ladybug_import_document() {
        let mut openai = Entity::new("openai", "OpenAI", EntityType::Organization);
        openai.description = Some("AI lab".into());
        let gpt4 = Entity::new("gpt4", "GPT-4", EntityType::Technology);

        let mut kg = crate::types::KnowledgeGraph::new();
        kg.add_entity(openai.clone());
        kg.add_entity(gpt4.clone());
        kg.add_triple(Triple::new(
            openai,
            Predicate::new(PredicateType::DevelopedBy),
            gpt4,
        ));

        let doc = to_ladybug_import_json(&kg);
        assert_eq!(doc["format_version"], "graphdb-ladybug.export.v1");
        assert!(doc["schema"][0]
            .as_str()
            .unwrap()
            .contains("CREATE NODE TABLE KgEntity"));
        assert!(doc["schema"].as_array().unwrap().iter().any(|s| s
            .as_str()
            .unwrap()
            .contains("CREATE REL TABLE DEVELOPED_BY")));
        assert_eq!(doc["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(doc["relationships"][0]["_type"], "DEVELOPED_BY");
        assert_eq!(doc["relationships"][0]["_from_table"], "KgEntity");
        assert_eq!(doc["relationships"][0]["predicate"], "DEVELOPED_BY");
    }
}
