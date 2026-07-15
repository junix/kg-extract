//! Conversion to shared `core-types-rs` knowledge graph protocol types.

use std::collections::{BTreeMap, HashMap};

use core_types_rs::{
    KgDocument, KgEntity, KgEvidence, KgRelation, LineSpan, SourceRange, KG_PROTOCOL_VERSION,
};
use serde_json::Value;

use crate::citation::CITATIONS_KEY;
use crate::types::{KnowledgeGraph, Triple};

impl KnowledgeGraph {
    /// Convert this extractor-domain graph into the portable KG protocol shape.
    pub fn to_kg_document(&self) -> KgDocument {
        KgDocument {
            schema_version: KG_PROTOCOL_VERSION.to_string(),
            entities: self
                .entities
                .iter()
                .map(|(_, entity)| KgEntity {
                    id: entity.id.clone(),
                    label: entity.label.clone(),
                    entity_type: entity.output_type(),
                    description: entity.description.clone(),
                    confidence: entity.confidence,
                    properties: entity_properties(entity),
                    evidence: citations_to_evidence(&entity.metadata),
                })
                .collect(),
            relations: self
                .triples
                .iter()
                .map(|triple| KgRelation {
                    id: None,
                    subject: triple.subject.id.clone(),
                    predicate: triple.predicate.output_type(),
                    object: triple.object.id.clone(),
                    label: triple.predicate.label.clone(),
                    confidence: triple.confidence.or(triple.predicate.confidence),
                    properties: relation_properties(triple),
                    evidence: citations_to_evidence(&triple.metadata),
                })
                .collect(),
            hyperedges: Vec::new(),
            schema: None,
            metadata: metadata_to_properties(&self.metadata),
        }
    }
}

fn entity_properties(entity: &crate::types::Entity) -> BTreeMap<String, Value> {
    let mut properties = record_metadata_to_properties(&entity.metadata);
    properties.insert(
        "normalized_entity_type".into(),
        Value::String(entity.entity_type.value()),
    );
    properties
}

fn relation_properties(triple: &Triple) -> BTreeMap<String, Value> {
    let mut properties = record_metadata_to_properties(&triple.metadata);
    properties.insert(
        "normalized_predicate_type".into(),
        Value::String(triple.predicate.predicate_type.value()),
    );
    if !triple.predicate.metadata.is_empty() {
        properties.insert(
            "predicate_metadata".into(),
            Value::Object(
                triple
                    .predicate
                    .metadata
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
        );
    }
    properties
}

fn record_metadata_to_properties(metadata: &HashMap<String, Value>) -> BTreeMap<String, Value> {
    let citations_promoted =
        parse_internal_citations(metadata).is_some_and(|evidence| !evidence.is_empty());
    metadata
        .iter()
        .filter(|(key, _)| !citations_promoted || key.as_str() != CITATIONS_KEY)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn metadata_to_properties(metadata: &HashMap<String, Value>) -> BTreeMap<String, Value> {
    metadata
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn citations_to_evidence(metadata: &HashMap<String, Value>) -> Vec<KgEvidence> {
    parse_internal_citations(metadata).unwrap_or_default()
}

fn parse_internal_citations(metadata: &HashMap<String, Value>) -> Option<Vec<KgEvidence>> {
    let Some(Value::Array(citations)) = metadata.get(CITATIONS_KEY) else {
        return None;
    };

    citations
        .iter()
        .map(internal_citation_to_evidence)
        .collect()
}

fn internal_citation_to_evidence(citation: &Value) -> Option<KgEvidence> {
    let object = citation.as_object()?;
    let source_file = match object.get("doc")? {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        _ => return None,
    };
    let range = if object.len() == 2 && object.contains_key("range") {
        object
            .get("range")
            .and_then(source_range_from_value)
            .filter(source_range_has_coordinates)?
    } else if object.len() == 2 && object.contains_key("lines") {
        object.get("lines").and_then(lines_to_source_range)?
    } else {
        return None;
    };
    Some(KgEvidence {
        source_file,
        source_id: None,
        range: Some(range),
        quote: None,
        metadata: BTreeMap::new(),
    })
}

fn source_range_has_coordinates(range: &SourceRange) -> bool {
    range.char_span.is_some()
        || range.line.is_some()
        || range.page.is_some()
        || range.bbox.is_some()
}

fn source_range_from_value(value: &Value) -> Option<SourceRange> {
    let object = value.as_object()?;
    if object.is_empty()
        || object
            .keys()
            .any(|key| !["char_span", "line", "page", "bbox"].contains(&key.as_str()))
    {
        return None;
    }
    for key in ["char_span", "line", "page"] {
        if object
            .get(key)
            .is_some_and(|span| !object_has_exact_keys(span, &["start", "end"]))
        {
            return None;
        }
    }
    if object
        .get("bbox")
        .is_some_and(|bbox| !object_has_exact_keys(bbox, &["x0", "y0", "x1", "y1"]))
    {
        return None;
    }
    serde_json::from_value(value.clone()).ok()
}

fn object_has_exact_keys(value: &Value, expected: &[&str]) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

fn lines_to_source_range(value: &Value) -> Option<SourceRange> {
    let lines = value.as_array()?;
    if lines.len() != 2 {
        return None;
    }
    let start = lines.first()?.as_u64()?;
    let end = lines.get(1)?.as_u64()?;
    let start = u32::try_from(start).ok()?;
    let end = u32::try_from(end).ok()?;
    let line = LineSpan::new(start, end)?;
    Some(SourceRange {
        char_span: None,
        line: Some(line),
        ..SourceRange::default()
    })
}

#[cfg(test)]
mod tests {
    use core_types_rs::KG_PROTOCOL_VERSION;
    use serde_json::json;

    use crate::citation::{attach_citation, Citation, CITATIONS_KEY};
    use crate::types::{Entity, EntityType, KnowledgeGraph, Predicate, PredicateType, Triple};

    #[test]
    fn knowledge_graph_converts_to_portable_kg_protocol() {
        let mut openai = Entity::new("entity_openai", "OpenAI", EntityType::Organization);
        openai.confidence = Some(0.9);
        openai.metadata.insert("alias".into(), json!("Open AI"));
        attach_citation(
            &mut openai.metadata,
            &Citation::new(Some("doc.md".into()), 3, 5),
        );
        let gpt4 = Entity::new("entity_gpt4", "GPT-4", EntityType::Technology);

        let mut predicate = Predicate::with_label(PredicateType::DevelopedBy, "developed by");
        predicate
            .metadata
            .insert("source".into(), json!("schema-json"));
        let mut triple = Triple::new(gpt4.clone(), predicate, openai.clone());
        triple.confidence = Some(0.8);

        let mut kg = KnowledgeGraph::new();
        kg.add_entity(openai);
        kg.add_entity(gpt4);
        kg.add_triple(triple);

        let doc = kg.to_kg_document();
        assert_eq!(doc.schema_version, KG_PROTOCOL_VERSION);
        assert_eq!(doc.entities.len(), 2);
        assert_eq!(doc.entities[0].entity_type, "ORGANIZATION");
        assert_eq!(
            doc.entities[0].properties["normalized_entity_type"],
            json!("ORGANIZATION")
        );
        assert_eq!(
            doc.entities[0].evidence[0].source_file.as_deref(),
            Some("doc.md")
        );
        assert_eq!(
            doc.entities[0].evidence[0]
                .range
                .as_ref()
                .unwrap()
                .line
                .unwrap()
                .start,
            3
        );
        assert_eq!(doc.relations.len(), 1);
        assert_eq!(doc.relations[0].subject, "entity_gpt4");
        assert_eq!(doc.relations[0].predicate, "developed by");
        assert_eq!(
            doc.relations[0].properties["normalized_predicate_type"],
            json!("DEVELOPED_BY")
        );
        assert_eq!(doc.relations[0].object, "entity_openai");
        assert!(doc.relations[0]
            .properties
            .contains_key("predicate_metadata"));
    }

    #[test]
    fn non_provenance_citations_metadata_is_not_silently_dropped() {
        let mut entity = Entity::new("entity_note", "Note", EntityType::Organization);
        entity
            .metadata
            .insert(CITATIONS_KEY.into(), json!("user-defined value"));
        let mut kg = KnowledgeGraph::new();
        kg.add_entity(entity);

        let doc = kg.to_kg_document();

        assert!(doc.entities[0].evidence.is_empty());
        assert_eq!(
            doc.entities[0].properties[CITATIONS_KEY],
            json!("user-defined value")
        );
    }

    #[test]
    fn foreign_citation_object_list_is_not_promoted_or_dropped() {
        let foreign = json!([{"user": "value"}]);
        let mut entity = Entity::new("entity_note", "Note", EntityType::Organization);
        entity
            .metadata
            .insert(CITATIONS_KEY.into(), foreign.clone());
        let mut kg = KnowledgeGraph::new();
        kg.add_entity(entity);

        let doc = kg.to_kg_document();

        assert!(doc.entities[0].evidence.is_empty());
        assert_eq!(doc.entities[0].properties[CITATIONS_KEY], foreign);
    }

    #[test]
    fn mixed_internal_and_foreign_citations_are_preserved_as_user_metadata() {
        let mixed = json!([
            {"doc": "doc.md", "lines": [1, 2]},
            {"user": "value"}
        ]);
        let mut entity = Entity::new("entity_note", "Note", EntityType::Organization);
        entity.metadata.insert(CITATIONS_KEY.into(), mixed.clone());
        let mut kg = KnowledgeGraph::new();
        kg.add_entity(entity);

        let doc = kg.to_kg_document();

        assert!(doc.entities[0].evidence.is_empty());
        assert_eq!(doc.entities[0].properties[CITATIONS_KEY], mixed);
    }

    #[test]
    fn rich_citation_with_foreign_nested_range_field_is_preserved() {
        let foreign = json!([{
            "doc": "doc.md",
            "range": {"line": {"start": 1, "end": 2}, "user": "value"}
        }]);
        let mut entity = Entity::new("entity_note", "Note", EntityType::Organization);
        entity
            .metadata
            .insert(CITATIONS_KEY.into(), foreign.clone());
        let mut kg = KnowledgeGraph::new();
        kg.add_entity(entity);

        let doc = kg.to_kg_document();

        assert!(doc.entities[0].evidence.is_empty());
        assert_eq!(doc.entities[0].properties[CITATIONS_KEY], foreign);
    }

    #[test]
    fn legacy_citation_with_extra_line_value_is_preserved() {
        let foreign = json!([{"doc": "doc.md", "lines": [1, 2, 3]}]);
        let mut entity = Entity::new("entity_note", "Note", EntityType::Organization);
        entity
            .metadata
            .insert(CITATIONS_KEY.into(), foreign.clone());
        let mut kg = KnowledgeGraph::new();
        kg.add_entity(entity);

        let doc = kg.to_kg_document();

        assert!(doc.entities[0].evidence.is_empty());
        assert_eq!(doc.entities[0].properties[CITATIONS_KEY], foreign);
    }
}
