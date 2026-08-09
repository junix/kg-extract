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

// ---------------------------------------------------------------------------
// extract_json_from_response: the three-tier fallback chain
// (```json fence -> ``` fence -> whole string) and its fall-through semantics.
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// parse_entities_and_triples: branch coverage for the three entity shapes and
// the two relationship shapes, including every skip path.
// ---------------------------------------------------------------------------

#[test]
fn parse_entities_object_with_scalar_value_uses_value_as_label() {
    // The object-entity branch falls back to value_to_string when an entity
    // value is not itself an object (e.g. a bare string id->label mapping).
    let json = serde_json::json!({"entities": {"e1": "Just a string"}});
    let (entities, rels) = parse_entities_and_triples(&json);
    assert!(rels.is_empty());
    assert_eq!(entities.len(), 1);
    assert_eq!(entities["e1"].label, "Just a string");
    assert!(entities["e1"].r#type.is_none());
    assert!(entities["e1"].description.is_none());
}

#[test]
fn parse_entities_array_uses_name_field_when_no_label() {
    // Array entities accept `name` as the label source when `label` is absent.
    let json = serde_json::json!({"entities": [{"name": "From Name Field"}]});
    let (entities, _rels) = parse_entities_and_triples(&json);
    assert_eq!(entities.len(), 1);
    let e = entities.values().next().unwrap();
    assert_eq!(e.label, "From Name Field");
}

#[test]
fn parse_relationships_object_form_accepts_source_subject_aliases_in_order() {
    // Object relationships accept {source|subject, target|object, predicate|relation};
    // a tuple missing `target` is skipped. Ordering of the surviving tuples is
    // preserved.
    let json = serde_json::json!({
        "entities": {
            "a": {"type": "OTHER"}, "b": {"type": "OTHER"},
            "c": {"type": "OTHER"}, "d": {"type": "OTHER"}
        },
        "relationships": [
            {"source": "a", "predicate": "USES", "target": "b"},
            {"subject": "c", "relation": "DEVELOPED_BY", "object": "d"},
            {"source": "a", "predicate": "USES"}
        ]
    });
    let (_entities, rels) = parse_entities_and_triples(&json);
    assert_eq!(
        rels,
        vec![
            ("a".to_string(), "USES".to_string(), "b".to_string()),
            ("c".to_string(), "DEVELOPED_BY".to_string(), "d".to_string()),
        ]
    );
}

#[test]
fn parse_relationships_array_form_requires_at_least_three_elements() {
    let json = serde_json::json!({
        "entities": {"a": {}, "b": {}, "c": {}, "d": {}, "e": {}},
        "relationships": [
            ["a", "USES", "b"],            // exactly 3 -> kept
            ["a", "b"],                     // too short -> skipped
            ["c", "DEVELOPED_BY", "d", "extra"], // >=3 -> first 3 kept, rest ignored
            []                              // empty -> skipped
        ]
    });
    let (_entities, rels) = parse_entities_and_triples(&json);
    assert_eq!(
        rels,
        vec![
            ("a".to_string(), "USES".to_string(), "b".to_string()),
            ("c".to_string(), "DEVELOPED_BY".to_string(), "d".to_string()),
        ]
    );
}

// ---------------------------------------------------------------------------
// create_triples_from_parsed: the dangling-endpoint skip contract.
// ---------------------------------------------------------------------------

#[test]
fn create_triples_skips_relationships_with_unknown_endpoints() {
    // Only `a` is a known entity; relationships referencing `b` (unknown) on
    // either side are dropped, the fully-resolved one is kept.
    let mut entities_info = HashMap::new();
    entities_info.insert(
        "a".to_string(),
        EntityInfo {
            label: "A".to_string(),
            ..Default::default()
        },
    );
    let entities = create_entities_from_parsed(&entities_info);
    let relationships: Vec<RelTuple> = vec![
        ("a".to_string(), "USES".to_string(), "a".to_string()), // both known
        ("a".to_string(), "USES".to_string(), "b".to_string()), // unknown object
        ("b".to_string(), "USES".to_string(), "a".to_string()), // unknown subject
    ];
    let triples = create_triples_from_parsed(&relationships, &entities);
    assert_eq!(triples.len(), 1, "only the fully-resolved triple survives");
    assert_eq!(triples[0].subject.label, "A");
    assert_eq!(triples[0].object.label, "A");
}

// ---------------------------------------------------------------------------
// parse_llm_response: the no-JSON error path records a parse_error diagnostic
// and yields an empty (not panicked) result.
// ---------------------------------------------------------------------------

#[test]
fn parse_llm_response_records_parse_error_when_no_json() {
    let parsed = parse_llm_response("the model emitted prose, no JSON at all");
    assert!(
        parsed.entities.is_empty(),
        "no JSON -> no entities (not a panic)"
    );
    assert!(parsed.relationships.is_empty());
    let err = parsed
        .metadata
        .get("parse_error")
        .expect("a parse failure must record a parse_error diagnostic");
    assert!(
        err.as_str().unwrap_or_default().contains("No JSON found"),
        "parse_error must carry the documented message: {err}"
    );
    assert!(
        !parsed.metadata.contains_key("raw_json"),
        "raw_json must not be present when extraction failed"
    );
}

// ---------------------------------------------------------------------------
// Legacy entities_and_triples: the malformed-entry skip paths.
// ---------------------------------------------------------------------------

#[test]
fn parse_legacy_entities_and_triples_skips_malformed_entries() {
    let json: serde_json::Value = serde_json::json!({
        "entities_and_triples": [
            "[1], OpenAI",          // 1 marker, valid -> entity
            "[2]",                   // 1 marker, no ", " separator -> skipped
            "[1] developed_by [2]",  // 2 markers -> relationship
            "[3] a [4] b [5]",       // 3 markers -> skipped (only 1 or 2 handled)
            42,                      // non-string item -> skipped
            "[4], GPT-4"             // 1 marker, valid -> entity
        ]
    });
    let (entities, relationships) = parse_entities_and_triples(&json);
    assert_eq!(entities.len(), 2);
    assert_eq!(entities["[1]"].label, "OpenAI");
    assert_eq!(entities["[4]"].label, "GPT-4");
    assert!(!entities.contains_key("[2]"), "marker without a label is skipped");
    assert_eq!(
        relationships,
        vec![("[1]".to_string(), "developed_by".to_string(), "[2]".to_string())]
    );
}
