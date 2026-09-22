use super::*;
use crate::backend::MockBackend;
use crate::graph_build::entity_id;

#[tokio::test]
async fn schema_based_extraction() {
    let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"},
                                  "GPT-4": {"type": "TECHNOLOGY"}},
                    "relationships": [["GPT-4", "DEVELOPED_BY", "OpenAI"]]}"#;
    let backend = Arc::new(MockBackend::single(json));
    let ex = SchemaJsonExtractor::new(backend);
    let out = ex.extract("OpenAI developed GPT-4.").await.unwrap();
    assert_eq!(out.num_entities(), 2);
    assert_eq!(out.num_triples(), 1);
    assert_eq!(out.knowledge_graph.triples[0].subject.label, "GPT-4");
    assert_eq!(out.knowledge_graph.triples[0].object.label, "OpenAI");
}

#[tokio::test]
async fn open_schema_output_preserves_raw_entity_and_relation_types() {
    let json = r#"{"entities": {
        "KG-RAG": {"type": "METHOD"},
        "RAG": {"type": "FRAMEWORK"}
    }, "relationships": [["KG-RAG", "BUILDS_ON", "RAG"]]}"#;
    let out = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
        .extract("KG-RAG builds on RAG.")
        .await
        .unwrap();

    let doc = out.knowledge_graph.to_dict();
    let kg_rag_id = entity_id("KG-RAG");
    assert_eq!(doc["entities"][&kg_rag_id]["type"], "METHOD");
    assert_eq!(
        doc["entities"][&kg_rag_id]["normalized_type"],
        "PHYSICAL_OBJECT"
    );
    assert_eq!(doc["triples"][0]["predicate"]["type"], "BUILDS_ON");
    assert_eq!(
        doc["triples"][0]["predicate"]["normalized_type"],
        "RELATED_TO"
    );
}

#[tokio::test]
async fn object_relationships_preserve_attributes_as_triple_metadata() {
    let json = r#"{"entities": {
        "KG-RAG": {"type": "METHOD", "evidence_quote": "KG-RAG framework"},
        "SPOKE": {"type": "KNOWLEDGE_GRAPH", "attributes": {"role": "knowledge source"}}
    }, "relationships": [{
        "source": "KG-RAG",
        "type": "RETRIEVES_FROM",
        "target": "SPOKE",
        "evidence_quote": "retrieves context from SPOKE",
        "source_section": "Methods"
    }]}"#;
    let out = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
        .extract("KG-RAG retrieves context from SPOKE.")
        .await
        .unwrap();

    let doc = out.knowledge_graph.to_dict();
    assert_eq!(doc["triples"][0]["predicate"]["type"], "RETRIEVES_FROM");
    assert_eq!(
        doc["triples"][0]["metadata"]["evidence_quote"],
        "retrieves context from SPOKE"
    );
    assert_eq!(doc["triples"][0]["metadata"]["source_section"], "Methods");
    let kg_rag_id = entity_id("KG-RAG");
    assert_eq!(
        doc["entities"][&kg_rag_id]["metadata"]["evidence_quote"],
        "KG-RAG framework"
    );
}

/// Single-shot engines never chunk, so pre-chunked input goes through the
/// trait's default: the chunk texts are joined and extracted in one call,
/// exactly like plain-text input.
#[tokio::test]
async fn prechunked_default_joins_chunks_into_one_call() {
    use crate::chunking::Segment;
    let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"}}, "relationships": []}"#;
    let backend = Arc::new(MockBackend::single(json));
    let ex = SchemaJsonExtractor::new(backend.clone());

    let chunks = vec![
        Segment {
            content: "OpenAI is an AI lab.".into(),
            index: 0,
            start: 0,
            end: 20,
            range: Some(core_types_rs::SourceRange {
                line: core_types_rs::LineSpan::new(1, 1),
                ..core_types_rs::SourceRange::default()
            }),
            title: None,
            metadata: std::collections::BTreeMap::new(),
        },
        Segment {
            content: "It developed GPT-4.".into(),
            index: 1,
            start: 20,
            end: 39,
            range: Some(core_types_rs::SourceRange {
                line: core_types_rs::LineSpan::new(2, 2),
                ..core_types_rs::SourceRange::default()
            }),
            title: None,
            metadata: std::collections::BTreeMap::new(),
        },
    ];
    let out = ex.extract_prechunked(&chunks).await.unwrap();
    assert_eq!(out.num_entities(), 1);

    let prompts = backend.seen_prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1, "single-shot: exactly one LLM call");
    assert!(
        prompts[0].contains("OpenAI is an AI lab.\n\nIt developed GPT-4."),
        "chunks must be joined into the user turn: {}",
        prompts[0]
    );
}

#[tokio::test]
async fn within_response_duplicates_honor_merge_strategy() {
    // The model emits the same entity twice (different casing) with different
    // descriptions. The configured merge_strategy must govern how they fold —
    // not a hardcoded first-wins.
    let json = r#"{"entities": {
        "OpenAI": {"type": "ORGANIZATION", "attributes": {"description": "first"}},
        "openai": {"type": "ORGANIZATION", "attributes": {"description": "second"}}
    }, "relationships": []}"#;
    let desc_of = |r: &ExtractionResponse| {
        r.knowledge_graph
            .entities
            .values()
            .next()
            .unwrap()
            .description
            .clone()
    };

    // Default KeepExisting: first occurrence wins (historical behaviour).
    let keep = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
        .extract("OpenAI")
        .await
        .unwrap();
    assert_eq!(keep.num_entities(), 1);
    assert_eq!(desc_of(&keep).as_deref(), Some("first"));

    // KeepIncoming: the later duplicate's data must replace it — proving the
    // strategy is actually applied in the schema-json path.
    let spec = ExtractionSpec {
        merge_strategy: MergeStrategy::KeepIncoming,
        ..Default::default()
    };
    let inc = SchemaJsonExtractor::with_spec(Arc::new(MockBackend::single(json)), spec)
        .extract("OpenAI")
        .await
        .unwrap();
    assert_eq!(inc.num_entities(), 1);
    assert_eq!(
        desc_of(&inc).as_deref(),
        Some("second"),
        "merge_strategy must take effect on within-response duplicates"
    );
}

#[tokio::test]
async fn template_extracts_under_fixed_mode_without_a_schema() {
    // A preset drives the prompt itself, so `--schema-mode fixed` (which
    // otherwise demands a non-empty schema) must NOT reject a template-only
    // spec with an empty schema — the template path ignores the schema.
    use crate::template::gallery;
    let tpl = gallery::get("general/concept_graph").expect("concept_graph preset");
    let spec = ExtractionSpec::from_template(tpl, Some("en".into()));
    let json = r#"{"entities": {"Photosynthesis": {"type": "PROCESS"}}, "relationships": []}"#;
    let out = SchemaJsonExtractor::with_spec(Arc::new(MockBackend::single(json)), spec)
        .schema_mode(SchemaMode::Fixed)
        .extract("Photosynthesis is a process.")
        .await
        .expect("template-driven extraction must succeed despite Fixed mode + empty schema");
    assert_eq!(out.num_entities(), 1);
}

#[tokio::test]
async fn relationship_resolves_case_insensitively() {
    // Entities are "OpenAI"/"GPT-4" but the relationship references them in a
    // different case; the edge must still be created, not silently dropped.
    let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"}, "GPT-4": {"type": "TECHNOLOGY"}},
                   "relationships": [["openai", "uses", "gpt-4"]]}"#;
    let out = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
        .extract("text")
        .await
        .unwrap();
    assert_eq!(out.num_entities(), 2);
    assert_eq!(
        out.num_triples(),
        1,
        "relationship must resolve despite case mismatch"
    );
}

#[tokio::test]
async fn entity_ids_are_deterministic_md5() {
    let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"}}, "relationships": []}"#;
    let a = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
        .extract("text")
        .await
        .unwrap();
    let b = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
        .extract("text")
        .await
        .unwrap();
    let ka: Vec<&String> = a.knowledge_graph.entities.keys().collect();
    let kb: Vec<&String> = b.knowledge_graph.entities.keys().collect();
    assert_eq!(
        ka, kb,
        "SchemaJson entity ids must be deterministic across runs"
    );
    let expected = entity_id("OpenAI");
    assert!(
        a.knowledge_graph.entities.contains_key(&expected),
        "id must follow the shared md5(name) scheme"
    );
}

#[tokio::test]
async fn open_is_the_default_and_needs_no_schema() {
    // Default mode is Open: an empty schema is fine, model extracts freely.
    let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"}}, "relationships": []}"#;
    let out = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
        .extract("text")
        .await
        .unwrap();
    assert_eq!(out.metadata["schema_mode"], serde_json::json!("open"));
    assert_eq!(out.metadata["mode"], serde_json::json!("schema_json"));
    assert_eq!(out.num_entities(), 1);
}

#[tokio::test]
async fn evolving_mode_captures_new_schema() {
    let json = r#"{"entities": {"Movie X": {"type": "WORK_OF_ART"}},
                    "relationships": [],
                    "new_schema_types": {"nodes": ["Movie"], "relations": ["starring"], "attributes": []}}"#;
    // Evolving requires a non-empty seed schema.
    let cfg = ExtractionConfig::from_schema(Schema::new(
        vec!["WORK_OF_ART".into()],
        vec!["RELATED_TO".into()],
        vec![],
    ));
    let ex = SchemaJsonExtractor::with_config(Arc::new(MockBackend::single(json)), cfg)
        .schema_mode(SchemaMode::Evolving);
    let out = ex.extract("Some text about a movie.").await.unwrap();
    assert!(out.metadata.contains_key("new_schema_types"));
    assert_eq!(out.metadata["schema_mode"], serde_json::json!("evolving"));
}

#[tokio::test]
async fn fixed_mode_drops_out_of_schema_records() {
    // Schema allows ORGANIZATION nodes and DEVELOPED_BY relations only. The
    // model leaks a TECHNOLOGY entity and a USES relation. Fixed mode must now
    // hard-drop them (not just prompt against them), and the DEVELOPED_BY
    // relation must also go because its GPT-4 endpoint was dropped.
    let json = r#"{"entities": {
        "OpenAI": {"type": "ORGANIZATION"},
        "GPT-4": {"type": "TECHNOLOGY"}
    }, "relationships": [
        ["GPT-4", "DEVELOPED_BY", "OpenAI"],
        ["OpenAI", "USES", "OpenAI"]
    ]}"#;
    let cfg = ExtractionConfig::from_schema(Schema::new(
        vec!["ORGANIZATION".into()],
        vec!["DEVELOPED_BY".into()],
        vec![],
    ));
    let out = SchemaJsonExtractor::with_config(Arc::new(MockBackend::single(json)), cfg)
        .schema_mode(SchemaMode::Fixed)
        .extract("text")
        .await
        .unwrap();
    assert_eq!(
        out.num_entities(),
        1,
        "only the ORGANIZATION entity survives"
    );
    assert_eq!(
        out.num_triples(),
        0,
        "USES is out-of-schema; DEVELOPED_BY loses its endpoint"
    );
    // 1 entity (TECHNOLOGY) + 2 relations (USES type, DEVELOPED_BY endpoint).
    assert_eq!(out.metadata["schema_dropped_records"], serde_json::json!(3));
    let dropped = out.metadata["schema_dropped_types"].as_array().unwrap();
    assert!(dropped.contains(&serde_json::json!("TECHNOLOGY")));
    assert!(dropped.contains(&serde_json::json!("USES")));
}

#[tokio::test]
async fn fixed_mode_keeps_in_schema_records() {
    // Everything is in-schema → nothing dropped, the drop metadata reports 0.
    let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"}, "GPT-4": {"type": "TECHNOLOGY"}},
                   "relationships": [["GPT-4", "DEVELOPED_BY", "OpenAI"]]}"#;
    let cfg = ExtractionConfig::from_schema(Schema::new(
        vec!["ORGANIZATION".into(), "TECHNOLOGY".into()],
        vec!["DEVELOPED_BY".into()],
        vec![],
    ));
    let out = SchemaJsonExtractor::with_config(Arc::new(MockBackend::single(json)), cfg)
        .schema_mode(SchemaMode::Fixed)
        .extract("text")
        .await
        .unwrap();
    assert_eq!(out.num_entities(), 2);
    assert_eq!(out.num_triples(), 1);
    assert_eq!(out.knowledge_graph.triples[0].subject.label, "GPT-4");
    assert_eq!(out.knowledge_graph.triples[0].object.label, "OpenAI");
    assert_eq!(out.metadata["schema_dropped_records"], serde_json::json!(0));
}

#[tokio::test]
async fn open_mode_does_not_enforce_schema() {
    // The same leak under Open mode must pass through untouched (no drop
    // metadata at all) — enforcement is Fixed-only.
    let json = r#"{"entities": {"GPT-4": {"type": "TECHNOLOGY"}}, "relationships": []}"#;
    let out = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
        .extract("text")
        .await
        .unwrap();
    assert_eq!(out.num_entities(), 1);
    assert!(!out.metadata.contains_key("schema_dropped_records"));
}

#[tokio::test]
async fn fixed_mode_without_schema_errors() {
    // Fixed on an empty schema is the degenerate combo — must error, not
    // silently tell the model to "use only types from []".
    let err = SchemaJsonExtractor::new(Arc::new(MockBackend::single("{}")))
        .schema_mode(SchemaMode::Fixed)
        .extract("text")
        .await;
    assert!(err.is_err(), "Fixed mode with an empty schema must error");
}

#[test]
fn one_spec_runs_through_both_engines() {
    use crate::extractor::ToolCallExtractor;
    // A single declarative spec configures either mechanism (with_spec) —
    // the spec/execution split: define the contract once, pick the executor.
    let spec = ExtractionSpec::new(
        Schema::new(
            vec!["ORGANIZATION".into()],
            vec!["DEVELOPED_BY".into()],
            vec![],
        ),
        SchemaMode::Fixed,
    );
    let sj = SchemaJsonExtractor::with_spec(Arc::new(MockBackend::single("{}")), spec.clone());
    let tool = ToolCallExtractor::with_spec(Arc::new(MockBackend::single("{}")), spec.clone());
    assert_eq!(
        sj.config().spec,
        spec,
        "SchemaJson must carry the spec verbatim"
    );
    assert_eq!(
        tool.config().spec,
        spec,
        "ToolCall must carry the same spec"
    );
    // Execution params stay engine-specific (both default to qwen-max here,
    // but the segment sizes differ: 3000 vs 5000).
    assert_eq!(sj.config().segment_size, 3000);
    assert_eq!(tool.config().segment_size, 5000);
}

// -----------------------------------------------------------------------
// Dedup / coref parity with ToolCall.
//
// SchemaJson used to build its graph and return it without the
// `dedup_graph_coref` pass ToolCall runs. `GraphBuilder` dedups entities by
// lowercased name, but `KnowledgeGraph::add_triple` is a bare push — so a
// model emitting the same relation twice produced two identical triples,
// and `spec.coref` had no reader on this engine at all (`--coref` was a
// silent no-op).
// -----------------------------------------------------------------------

#[tokio::test]
async fn identical_relations_in_one_response_collapse_to_one_triple() {
    // The same relation twice — a real LLM failure mode, not a synthetic one.
    let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"},
                                 "GPT-4": {"type": "TECHNOLOGY"}},
                   "relationships": [["GPT-4", "DEVELOPED_BY", "OpenAI"],
                                     ["GPT-4", "DEVELOPED_BY", "OpenAI"]]}"#;
    let out = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
        .extract("OpenAI developed GPT-4.")
        .await
        .unwrap();
    assert_eq!(out.num_triples(), 1, "identical triples must collapse");
    assert_eq!(out.num_entities(), 2);
}

#[tokio::test]
async fn coref_fuzzy_merges_surface_variants() {
    // `Acme Corp.` / `Acme Corp` differ only by punctuation: Off keeps both,
    // Fuzzy collapses them. Proves spec.coref is actually read.
    let json = r#"{"entities": {"Acme Corp.": {"type": "ORGANIZATION"},
                                 "Acme Corp": {"type": "ORGANIZATION"},
                                 "Widget": {"type": "PRODUCT"}},
                   "relationships": [["Acme Corp.", "PRODUCES", "Widget"]]}"#;

    let off = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
        .extract("Acme Corp. produces Widget.")
        .await
        .unwrap();

    let spec = ExtractionSpec {
        coref: crate::types::CorefMode::Fuzzy,
        ..Default::default()
    };
    let fuzzy = SchemaJsonExtractor::with_spec(Arc::new(MockBackend::single(json)), spec)
        .extract("Acme Corp. produces Widget.")
        .await
        .unwrap();

    assert!(
        fuzzy.num_entities() < off.num_entities(),
        "CorefMode::Fuzzy must merge the Acme surface variants \
         (off={} entities, fuzzy={})",
        off.num_entities(),
        fuzzy.num_entities()
    );
}
