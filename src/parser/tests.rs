use super::*;

#[test]
fn parse_fenced_json() {
    let response = "```json\n{\"entities\": {\"e1\": {\"label\": \"GPT-4\", \"type\": \"technology\"}}, \"relationships\": [[\"e1\", \"uses\", \"e1\"]]}\n```";
    let parsed = parse_llm_response(response);
    let info: HashMap<String, EntityInfo> =
        serde_json::from_value(parsed.metadata["entities_info"].clone()).unwrap();
    let entities = create_entities_from_parsed(&info);
    assert_eq!(
        entities["e1"].entity_type,
        crate::types::EntityType::Technology
    );
    let triples = create_triples_from_parsed(&parsed.relationships, &entities);
    assert_eq!(triples.len(), 1);
    assert_eq!(
        triples[0].predicate.predicate_type,
        crate::types::PredicateType::Uses
    );
}

#[test]
fn parse_llm_response_populates_entities() {
    let response = r#"```json
        {"entities": {"e1": {"label": "GPT-4", "type": "technology"}}, "relationships": []}
        ```"#;
    let parsed = parse_llm_response(response);
    assert!(!parsed.entities.is_empty(), "entities must be populated");
    assert_eq!(parsed.entities["e1"].label, "GPT-4");
    assert_eq!(
        parsed.entities["e1"].entity_type,
        crate::types::EntityType::Technology
    );
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
    let (entities, relationships) = parse_entities_and_triples(&json);
    assert_eq!(entities.len(), 2);
    assert_eq!(entities["[1]"].label, "OpenAI");
    assert_eq!(
        relationships,
        vec![(
            "[1]".to_string(),
            "developed_by".to_string(),
            "[2]".to_string()
        )]
    );
    let built = create_entities_from_parsed(&entities);
    let triples = create_triples_from_parsed(&relationships, &built);
    assert_eq!(triples.len(), 1);
    assert_eq!(
        triples[0].predicate.predicate_type,
        crate::types::PredicateType::DevelopedBy
    );
}

#[test]
fn generated_entity_ids_do_not_overwrite_explicit_ids() {
    let json = serde_json::json!({
        "entities": [
            {"id": "entity_1", "name": "Explicit"},
            {"name": "Generated"}
        ]
    });

    let (entities, relationships) = parse_entities_and_triples(&json);

    assert!(relationships.is_empty());
    assert_eq!(entities.len(), 2);
    assert_eq!(entities["entity_1"].label, "Explicit");
    assert!(entities.values().any(|entity| entity.label == "Generated"));
}
