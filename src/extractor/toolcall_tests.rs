use super::*;
use crate::backend::MockBackend;
use crate::types::PredicateType;

fn call(name: &str, args: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        id: format!("c_{name}"),
        name: name.into(),
        arguments: args,
    }
}

#[tokio::test]
async fn single_round_collects_tool_calls() {
    let rounds = vec![vec![
        call(
            "add_entity",
            serde_json::json!({"name": "OpenAI", "type": "ORGANIZATION"}),
        ),
        call(
            "add_entity",
            serde_json::json!({"name": "GPT-4", "type": "TECHNOLOGY"}),
        ),
        call(
            "add_relation",
            serde_json::json!({"source": "GPT-4", "predicate": "DEVELOPED_BY", "target": "OpenAI", "strength": 0.9}),
        ),
        call(
            "add_attribute",
            serde_json::json!({"entity": "GPT-4", "key": "params", "value": "1.8T"}),
        ),
        call("finish", serde_json::json!({})),
    ]];
    let backend = Arc::new(MockBackend::new(vec![]).with_tool_rounds(rounds));
    let ex = ToolCallExtractor::new(backend);
    let out = ex.extract("OpenAI developed GPT-4.").await.unwrap();
    assert_eq!(out.num_entities(), 2);
    assert_eq!(out.num_triples(), 1);
    assert_eq!(
        out.knowledge_graph.triples[0].predicate.predicate_type,
        PredicateType::DevelopedBy
    );
    assert_eq!(out.knowledge_graph.triples[0].subject.label, "GPT-4");
    assert_eq!(out.knowledge_graph.triples[0].object.label, "OpenAI");
    let gpt = out
        .knowledge_graph
        .entities
        .values()
        .find(|e| e.label == "GPT-4")
        .unwrap();
    assert_eq!(gpt.metadata["params"], serde_json::json!("1.8T"));
}

#[tokio::test]
async fn canonical_direction_flips_and_dedups_direction_variants() {
    // spec.canonical_direction = true: (A, USES, B) and (B, IS_USED_BY, A)
    // converge on the canonical IS_USED_BY edge before dedup, so the output
    // holds ONE triple.
    let rounds = vec![vec![
        call(
            "add_entity",
            serde_json::json!({"name": "A", "type": "OTHER"}),
        ),
        call(
            "add_entity",
            serde_json::json!({"name": "B", "type": "OTHER"}),
        ),
        call(
            "add_relation",
            serde_json::json!({"source": "A", "predicate": "USES", "target": "B"}),
        ),
        call(
            "add_relation",
            serde_json::json!({"source": "B", "predicate": "IS_USED_BY", "target": "A"}),
        ),
    ]];
    let backend = Arc::new(MockBackend::new(vec![]).with_tool_rounds(rounds));
    let mut c = ToolCallExtractor::default_config();
    c.spec.canonical_direction = true;
    let out = ToolCallExtractor::with_config(backend, c)
        .extract("text")
        .await
        .unwrap();
    assert_eq!(
        out.num_triples(),
        1,
        "direction variants collapse to one canonical edge"
    );
    let t = &out.knowledge_graph.triples[0];
    assert_eq!(t.predicate.predicate_type, PredicateType::IsUsedBy);
    assert_eq!(t.subject.label, "B");
    assert_eq!(t.object.label, "A");
}

#[tokio::test]
async fn direction_variants_stay_separate_by_default() {
    // Default (canonical_direction off): the two direction variants remain
    // two edges — the opt-in must not change default behaviour.
    let rounds = vec![vec![
        call(
            "add_entity",
            serde_json::json!({"name": "A", "type": "OTHER"}),
        ),
        call(
            "add_entity",
            serde_json::json!({"name": "B", "type": "OTHER"}),
        ),
        call(
            "add_relation",
            serde_json::json!({"source": "A", "predicate": "USES", "target": "B"}),
        ),
        call(
            "add_relation",
            serde_json::json!({"source": "B", "predicate": "IS_USED_BY", "target": "A"}),
        ),
    ]];
    let backend = Arc::new(MockBackend::new(vec![]).with_tool_rounds(rounds));
    let out = ToolCallExtractor::new(backend)
        .extract("text")
        .await
        .unwrap();
    assert_eq!(out.num_triples(), 2);
}

#[tokio::test]
async fn relation_strength_is_clamped() {
    let rounds = vec![vec![
        call(
            "add_entity",
            serde_json::json!({"name": "A", "type": "OTHER"}),
        ),
        call(
            "add_entity",
            serde_json::json!({"name": "B", "type": "OTHER"}),
        ),
        call(
            "add_relation",
            serde_json::json!({"source": "A", "predicate": "USES", "target": "B", "strength": 5.0}),
        ),
    ]];
    let backend = Arc::new(MockBackend::new(vec![]).with_tool_rounds(rounds));
    let out = ToolCallExtractor::new(backend)
        .extract("text")
        .await
        .unwrap();
    assert_eq!(
        out.knowledge_graph.triples[0].confidence,
        Some(1.0),
        "strength clamps to 1.0"
    );
}

#[tokio::test]
async fn dangling_relation_is_dropped() {
    let rounds = vec![vec![
        call(
            "add_entity",
            serde_json::json!({"name": "OpenAI", "type": "ORGANIZATION"}),
        ),
        call(
            "add_relation",
            serde_json::json!({"source": "OpenAI", "predicate": "USES", "target": "Nonexistent"}),
        ),
    ]];
    let backend = Arc::new(MockBackend::new(vec![]).with_tool_rounds(rounds));
    let out = ToolCallExtractor::new(backend)
        .extract("text")
        .await
        .unwrap();
    assert_eq!(out.num_entities(), 1);
    assert_eq!(out.num_triples(), 0);
}

#[tokio::test]
async fn evolving_mode_records_proposed_types() {
    let rounds = vec![vec![
        call(
            "add_entity",
            serde_json::json!({"name": "Dune", "type": "WORK_OF_ART"}),
        ),
        call(
            "propose_schema_type",
            serde_json::json!({"kind": "node", "name": "Movie"}),
        ),
    ]];
    let backend = Arc::new(MockBackend::new(vec![]).with_tool_rounds(rounds));
    // Evolving requires a non-empty seed schema.
    let cfg = ExtractionConfig::from_schema(Schema::new(
        vec!["WORK_OF_ART".into()],
        vec!["RELATED_TO".into()],
        vec![],
    ));
    let out = ToolCallExtractor::with_config(backend, cfg)
        .schema_mode(SchemaMode::Evolving)
        .extract("text")
        .await
        .unwrap();
    assert_eq!(
        out.metadata["new_schema_types"]["nodes"][0],
        serde_json::json!("Movie")
    );
    assert_eq!(out.metadata["schema_mode"], serde_json::json!("evolving"));
}

#[tokio::test]
async fn fixed_mode_without_schema_errors() {
    // Fixed on an empty schema is the degenerate combo — must error.
    let backend = Arc::new(MockBackend::new(vec![]).with_tool_rounds(vec![vec![]]));
    let err = ToolCallExtractor::new(backend)
        .schema_mode(SchemaMode::Fixed)
        .extract("text")
        .await;
    assert!(err.is_err(), "Fixed mode with an empty schema must error");
}
