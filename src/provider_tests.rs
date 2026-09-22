use super::*;

fn smoke_document() -> Value {
    json!({
        "schema_version": "kg.protocol.v1",
        "entities": [
            {"id": "entity_a", "label": "OpenAI", "entity_type": "ORGANIZATION"},
            {"id": "entity_b", "label": "GPT-4", "entity_type": "TECHNOLOGY"}
        ],
        "relations": [
            {"subject": "entity_a", "predicate": "USES", "object": "entity_b",
             "evidence": [{"source_file": "doc.md", "range": {"line": {"start": 1, "end": 2}}}]}
        ]
    })
}

fn temp_artifacts(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kg-provider-test-{tag}-{}", nanoid::nanoid!()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---- describe ----------------------------------------------------------

#[test]
fn describe_manifest_is_self_consistent() {
    let doc = describe_document("0.0.0-test");
    assert_eq!(doc["protocol"], "kg.provider/v1");
    assert_eq!(doc["protocol_versions"], json!([1]));
    assert_eq!(doc["provider"]["id"], "kg-extract");
    assert_eq!(doc["provider"]["version"], "0.0.0-test");
    assert!(doc["provider"]["description"].as_str().unwrap().len() > 20);

    let caps = doc["capabilities"].as_array().unwrap();
    let ids: Vec<&str> = caps
        .iter()
        .map(|c| c["capability_id"].as_str().unwrap())
        .collect();
    // capability_id stability is the hub's match key — lock the set+order.
    assert_eq!(
        ids,
        vec![
            "extract.entities_relations",
            "detect.communities",
            "detect.communities_hierarchy",
            "summarize.communities",
            "resolve.coref",
            "resolve.canonical_direction",
        ]
    );

    for cap in caps {
        for key in ["capability_id", "title", "description", "side_effects", "input_schema", "output", "cli_spec"] {
            assert!(cap.get(key).is_some(), "capability missing {key}");
        }
        // Every input_schema property must carry a description that
        // survives losing the field name.
        let props = cap["input_schema"]["properties"].as_object().unwrap();
        for (name, prop) in props {
            let desc = prop["description"].as_str().unwrap_or("");
            assert!(
                desc.len() >= 20,
                "{}.{name} needs a real description, got: {desc:?}",
                cap["capability_id"]
            );
            // Enum-style properties use oneOf consts, each described.
            if let Some(one_of) = prop.get("oneOf") {
                for variant in one_of.as_array().unwrap() {
                    assert!(variant.get("const").is_some());
                    assert!(
                        variant["description"].as_str().unwrap_or("").len() >= 10,
                        "{}.{name} variant {:?} needs a description",
                        cap["capability_id"],
                        variant["const"]
                    );
                }
            }
        }
    }
}

#[test]
fn describe_side_effects_match_capability_nature() {
    let doc = describe_document("0.0.0");
    let caps = doc["capabilities"].as_array().unwrap();
    let by_id: std::collections::BTreeMap<&str, &Value> = caps
        .iter()
        .map(|c| (c["capability_id"].as_str().unwrap(), c))
        .collect();
    // LLM-calling capabilities declare egress; pure local ones declare none.
    assert_eq!(by_id[CAP_EXTRACT]["side_effects"], json!(["network", "data_egress"]));
    assert_eq!(by_id[CAP_SUMMARIZE]["side_effects"], json!(["network", "data_egress"]));
    for id in [CAP_DETECT_COMMUNITIES, CAP_DETECT_HIERARCHY, CAP_RESOLVE_COREF, CAP_RESOLVE_DIRECTION] {
        assert_eq!(by_id[id]["side_effects"], json!([]), "{id} must be side-effect free");
    }
    // Output contract per capability.
    assert_eq!(by_id[CAP_EXTRACT]["output"], json!({"mode": "artifact", "kind": "kg-document"}));
    assert_eq!(by_id[CAP_DETECT_COMMUNITIES]["output"], json!({"mode": "result-json", "kind": "communities"}));
    assert_eq!(by_id[CAP_DETECT_HIERARCHY]["output"], json!({"mode": "result-json", "kind": "communities"}));
    assert_eq!(by_id[CAP_RESOLVE_COREF]["output"], json!({"mode": "artifact", "kind": "kg-document"}));
}

#[test]
fn extract_cli_spec_flags_align_with_input_schema() {
    let doc = describe_document("0.0.0");
    let cap = &doc["capabilities"][0];
    assert_eq!(cap["capability_id"], CAP_EXTRACT);
    let props: std::collections::BTreeSet<&str> = cap["input_schema"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let flags: std::collections::BTreeSet<&str> = cap["cli_spec"]["flags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    // `text` is the only schema field without a flag — it rides stdin.
    let mut expected = props.clone();
    expected.remove("text");
    assert_eq!(flags, expected, "cli_spec flags must cover every schema field except stdin-fed `text`");
    // The extraction artifact format is pinned via `always`.
    assert_eq!(cap["cli_spec"]["always"], json!(["-o", "kg-protocol"]));
    // Orders are unique so rendering is deterministic.
    let mut orders: Vec<u64> = cap["cli_spec"]["flags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["order"].as_u64().unwrap())
        .collect();
    orders.sort_unstable();
    orders.dedup();
    assert_eq!(orders.len(), flags.len());
}

#[test]
fn graph_in_cli_specs_render_the_invoke_form() {
    let doc = describe_document("0.0.0");
    for cap in doc["capabilities"].as_array().unwrap()[1..].iter() {
        let id = cap["capability_id"].as_str().unwrap();
        assert_eq!(
            cap["cli_spec"]["subcommand"],
            json!(["invoke", id]),
            "{id}: graph-in capabilities have no flat-flag form; cli_spec must render invoke"
        );
        let flags = cap["cli_spec"]["flags"].as_array().unwrap();
        assert_eq!(flags[0]["name"], "request");
        assert_eq!(flags[0]["flag"], "--request");
        assert_eq!(flags[0]["default"], "-");
    }
}

// ---- available ----------------------------------------------------------

#[test]
fn available_report_shape_and_semantics() {
    let report = available_report();
    for key in ["available", "ready", "missing"] {
        assert!(report.get(key).is_some(), "missing key {key}");
    }
    assert!(report.get("cache_dir").is_none(), "no cache to report: cache_dir is omitted, not null");
    assert_eq!(
        report["available"].as_bool().unwrap(),
        report["missing"].as_array().unwrap().is_empty(),
        "available must equal 'nothing missing'"
    );
    // The mock backend is the one unconditional entry.
    assert!(report["ready"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["name"] == "backend:mock" && e["kind"] == "backend"));
    // Feature probes reflect the compiled feature set.
    let community_ready = report["ready"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["name"] == "feature:community");
    assert_eq!(community_ready, cfg!(feature = "community"));
}

// ---- invoke: error paths -------------------------------------------------

#[tokio::test]
async fn invoke_rejects_non_json_request() {
    let outcome = invoke(CAP_EXTRACT, "not json", None).await;
    assert!(!outcome.ok);
    assert_eq!(outcome.envelope["status"], "error");
    assert_eq!(outcome.envelope["error"]["code"], "invalid_request");
    assert_eq!(outcome.envelope["protocol"], "kg.execution/v1");
    assert_eq!(outcome.envelope["provider"], "kg-extract");
    assert_eq!(outcome.envelope["capability_id"], CAP_EXTRACT);
}

#[tokio::test]
async fn invoke_unknown_capability_is_machine_readable() {
    let outcome = invoke("nope.capability", "{}", None).await;
    assert!(!outcome.ok);
    assert_eq!(outcome.envelope["error"]["code"], "unknown_capability");
    assert!(outcome.envelope["error"]["message"]
        .as_str()
        .unwrap()
        .contains(CAP_EXTRACT));
}

#[tokio::test]
async fn invoke_extract_validates_input_contract() {
    // both text and file
    let outcome = invoke(
        CAP_EXTRACT,
        r#"{"text": "a", "file": "b.txt", "backend": "mock"}"#,
        None,
    )
    .await;
    assert_eq!(outcome.envelope["error"]["code"], "invalid_request");
    // neither
    let outcome = invoke(CAP_EXTRACT, r#"{"backend": "mock"}"#, None).await;
    assert_eq!(outcome.envelope["error"]["code"], "invalid_request");
    // unknown field (deny_unknown_fields — schema says additionalProperties: false)
    let outcome = invoke(CAP_EXTRACT, r#"{"text": "a", "nope": 1}"#, None).await;
    assert_eq!(outcome.envelope["error"]["code"], "invalid_request");
    assert!(outcome.envelope["error"]["message"]
        .as_str()
        .unwrap()
        .contains("unknown field"));
    // bad enum lists the valid values
    let outcome = invoke(
        CAP_EXTRACT,
        r#"{"text": "a", "backend": "mock", "engine": "nope"}"#,
        None,
    )
    .await;
    assert_eq!(outcome.envelope["error"]["code"], "invalid_request");
    assert!(outcome.envelope["error"]["message"]
        .as_str()
        .unwrap()
        .contains("schema-json"));
    // empty input text
    let outcome = invoke(CAP_EXTRACT, r#"{"text": "  ", "backend": "mock"}"#, None).await;
    assert_eq!(outcome.envelope["error"]["code"], "invalid_request");
}

#[cfg(not(feature = "llms-backend"))]
#[tokio::test]
async fn invoke_extract_reports_backend_unavailable() {
    let outcome = invoke(
        CAP_EXTRACT,
        r#"{"text": "OpenAI developed GPT-4.", "backend": "llms"}"#,
        None,
    )
    .await;
    assert!(!outcome.ok);
    assert_eq!(outcome.envelope["error"]["code"], "backend_unavailable");
    assert!(outcome.envelope["error"]["message"]
        .as_str()
        .unwrap()
        .contains("llms-backend"));
}

// ---- invoke: happy paths -------------------------------------------------

#[tokio::test]
async fn invoke_extract_mock_end_to_end() {
    let dir = temp_artifacts("extract");
    let request = json!({
        "text": "OpenAI developed GPT-4.",
        "engine": "simple",
        "backend": "mock",
        "mock_response": "(entity<|>OpenAI<|>organization<|>An AI research lab.<|>)##\n(entity<|>GPT-4<|>technology<|>A large language model.<|>)##\n(relationship<|>OpenAI<|>GPT-4<|>uses<|>OpenAI develops GPT-4.<|>0.9)##"
    });
    let outcome = invoke(CAP_EXTRACT, &request.to_string(), Some(&dir)).await;
    assert!(outcome.ok, "envelope: {}", outcome.envelope);
    assert_eq!(outcome.envelope["status"], "ok");
    assert_eq!(outcome.envelope["result"]["num_entities"], 2);
    assert_eq!(outcome.envelope["result"]["num_relations"], 1);

    // Artifact: exists, parses as kg.protocol.v1, checksum verifies.
    let artifact = &outcome.envelope["artifacts"][0];
    assert_eq!(artifact["kind"], "kg-document");
    let path = artifact["path"].as_str().unwrap();
    let body = std::fs::read_to_string(path).unwrap();
    let checksum = artifact["checksum"].as_str().unwrap();
    let expected = format!("sha256:{:x}", Sha256::digest(body.as_bytes()));
    assert_eq!(checksum, expected);
    let doc: core_types_rs::KgDocument = serde_json::from_str(&body).unwrap();
    assert_eq!(doc.schema_version, core_types_rs::KG_PROTOCOL_VERSION);
    assert_eq!(doc.entities.len(), 2);
    assert_eq!(doc.relations.len(), 1);

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "community")]
#[tokio::test]
async fn invoke_detect_communities_from_document() {
    let request = json!({"document": smoke_document()});
    let outcome = invoke(CAP_DETECT_COMMUNITIES, &request.to_string(), None).await;
    assert!(outcome.ok, "envelope: {}", outcome.envelope);
    assert_eq!(outcome.envelope["result"]["num_communities"], 1);
    assert!(outcome.envelope["result"]["quality"].is_null());
    assert_eq!(
        outcome.envelope["result"]["communities"]["0"],
        json!(["entity_a", "entity_b"])
    );
    assert!(outcome.envelope["artifacts"].as_array().unwrap().is_empty());
}

#[cfg(feature = "community")]
#[tokio::test]
async fn invoke_detect_communities_warns_on_dangling_relation() {
    let mut doc = smoke_document();
    doc["relations"]
        .as_array_mut()
        .unwrap()
        .push(json!({"subject": "entity_a", "predicate": "USES", "object": "ghost"}));
    let request = json!({"document": doc});
    let outcome = invoke(CAP_DETECT_COMMUNITIES, &request.to_string(), None).await;
    assert!(outcome.ok);
    let warnings: Vec<&str> = outcome.envelope["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["severity"] == "warning")
        .filter_map(|d| d["message"].as_str())
        .collect();
    assert!(
        warnings.iter().any(|m| m.contains("dangling") || m.contains("endpoint")),
        "expected a dangling-relation warning, got: {warnings:?}"
    );
}

#[cfg(feature = "community-leiden")]
#[tokio::test]
async fn invoke_detect_hierarchy_from_document() {
    let request = json!({"document": smoke_document()});
    let outcome = invoke(CAP_DETECT_HIERARCHY, &request.to_string(), None).await;
    assert!(outcome.ok, "envelope: {}", outcome.envelope);
    assert_eq!(outcome.envelope["result"]["detector"], "hierarchical-leiden");
    let levels = outcome.envelope["result"]["levels"].as_array().unwrap();
    assert!(!levels.is_empty());
    assert!(levels[0]["quality"].is_number());
}

#[cfg(feature = "community")]
#[tokio::test]
async fn invoke_summarize_communities_with_mock_backend() {
    let request = json!({
        "document": smoke_document(),
        "backend": "mock",
        "mock_response": "{\"name\": \"AI Stack\", \"summary\": \"OpenAI and its model.\"}"
    });
    let outcome = invoke(CAP_SUMMARIZE, &request.to_string(), None).await;
    assert!(outcome.ok, "envelope: {}", outcome.envelope);
    let community = &outcome.envelope["result"]["communities"]["0"];
    assert_eq!(community["name"], "AI Stack");
    assert_eq!(community["summary"], "OpenAI and its model.");
    assert_eq!(community["members"], json!(["entity_a", "entity_b"]));
}

#[tokio::test]
async fn invoke_resolve_coref_merges_surface_variants() {
    let dir = temp_artifacts("coref");
    let request = json!({
        "document": {
            "schema_version": "kg.protocol.v1",
            "entities": [
                {"id": "a", "label": "Acme, Inc.", "entity_type": "ORGANIZATION"},
                {"id": "b", "label": "Acme", "entity_type": "ORGANIZATION"}
            ],
            "relations": []
        }
    });
    let outcome = invoke(CAP_RESOLVE_COREF, &request.to_string(), Some(&dir)).await;
    assert!(outcome.ok, "envelope: {}", outcome.envelope);
    assert_eq!(outcome.envelope["result"]["num_entities"], 1);
    let artifact = &outcome.envelope["artifacts"][0];
    let body = std::fs::read_to_string(artifact["path"].as_str().unwrap()).unwrap();
    let doc: core_types_rs::KgDocument = serde_json::from_str(&body).unwrap();
    assert_eq!(doc.entities.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn invoke_resolve_canonical_direction_collapses_variants() {
    let dir = temp_artifacts("direction");
    let request = json!({
        "document": {
            "schema_version": "kg.protocol.v1",
            "entities": [
                {"id": "a", "label": "OpenAI", "entity_type": "ORGANIZATION"},
                {"id": "b", "label": "GPT-4", "entity_type": "TECHNOLOGY"}
            ],
            "relations": [
                {"subject": "a", "predicate": "USES", "object": "b"},
                {"subject": "b", "predicate": "IS_USED_BY", "object": "a"}
            ]
        }
    });
    let outcome = invoke(CAP_RESOLVE_DIRECTION, &request.to_string(), Some(&dir)).await;
    assert!(outcome.ok, "envelope: {}", outcome.envelope);
    assert_eq!(outcome.envelope["result"]["num_relations"], 1);
    let body = std::fs::read_to_string(outcome.envelope["artifacts"][0]["path"].as_str().unwrap()).unwrap();
    let doc: core_types_rs::KgDocument = serde_json::from_str(&body).unwrap();
    assert_eq!(doc.relations.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn invoke_resolve_rejects_missing_document() {
    let outcome = invoke(CAP_RESOLVE_COREF, "{}", None).await;
    assert!(!outcome.ok);
    assert_eq!(outcome.envelope["error"]["code"], "invalid_request");
    let outcome = invoke(
        CAP_RESOLVE_COREF,
        r#"{"document_file": "/no/such/doc-xyz.json"}"#,
        None,
    )
    .await;
    assert_eq!(outcome.envelope["error"]["code"], "invalid_request");
}

#[cfg(not(feature = "community"))]
#[tokio::test]
async fn invoke_detect_without_feature_reports_backend_unavailable() {
    let request = json!({"document": smoke_document()});
    let outcome = invoke(CAP_DETECT_COMMUNITIES, &request.to_string(), None).await;
    assert!(!outcome.ok);
    assert_eq!(outcome.envelope["error"]["code"], "backend_unavailable");
    assert!(outcome.envelope["error"]["message"]
        .as_str()
        .unwrap()
        .contains("--features community"));
}
