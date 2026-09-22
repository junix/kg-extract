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

#[test]
fn from_kg_document_round_trip_preserves_records_and_tokens() {
    let mut openai = Entity::new("entity_openai", "OpenAI", EntityType::Organization);
    openai.confidence = Some(0.9);
    openai.description = Some("An AI research lab.".into());
    attach_citation(
        &mut openai.metadata,
        &Citation::new(Some("doc.md".into()), 3, 5),
    );
    let gpt4 = Entity::new("entity_gpt4", "GPT-4", EntityType::Technology);
    let triple = Triple::new(
        gpt4.clone(),
        Predicate::with_label(PredicateType::DevelopedBy, "developed by"),
        openai.clone(),
    );

    let mut kg = KnowledgeGraph::new();
    kg.add_entity(openai);
    kg.add_entity(gpt4);
    kg.add_triple(triple);

    let (imported, report) = KnowledgeGraph::from_kg_document(&kg.to_kg_document());
    assert_eq!(report, crate::protocol::ImportReport::default());
    assert_eq!(imported.entities.len(), 2);
    assert_eq!(imported.triples.len(), 1);

    // Round-trip through the export again: tokens, confidence and evidence
    // ranges survive the detour through the internal citation shape.
    let doc = imported.to_kg_document();
    assert_eq!(doc.entities[0].entity_type, "ORGANIZATION");
    assert_eq!(doc.entities[0].confidence, Some(0.9));
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
    assert_eq!(doc.relations[0].predicate, "developed by");
    assert_eq!(
        doc.relations[0].properties["normalized_predicate_type"],
        json!("DEVELOPED_BY")
    );
}

#[test]
fn from_kg_document_drops_dangling_relations_with_a_count() {
    let doc: core_types_rs::KgDocument = serde_json::from_value(json!({
        "schema_version": "kg.protocol.v1",
        "entities": [{"id": "a", "label": "A", "entity_type": "ORGANIZATION"}],
        "relations": [
            {"subject": "a", "predicate": "USES", "object": "ghost"},
            {"subject": "ghost", "predicate": "USES", "object": "a"}
        ]
    }))
    .unwrap();
    let (kg, report) = KnowledgeGraph::from_kg_document(&doc);
    assert_eq!(kg.entities.len(), 1);
    assert!(kg.triples.is_empty());
    assert_eq!(report.dangling_relations, 2);
}

#[test]
fn from_kg_document_counts_range_less_evidence() {
    let doc: core_types_rs::KgDocument = serde_json::from_value(json!({
        "schema_version": "kg.protocol.v1",
        "entities": [{
            "id": "a", "label": "A", "entity_type": "ORGANIZATION",
            "evidence": [{"quote": "no range here"}]
        }],
        "relations": []
    }))
    .unwrap();
    let (kg, report) = KnowledgeGraph::from_kg_document(&doc);
    assert_eq!(kg.entities.len(), 1);
    assert_eq!(report.range_less_evidence, 1);
}
