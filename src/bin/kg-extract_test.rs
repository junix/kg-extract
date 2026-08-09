use super::*;
use kg_extract::provider::parse_mock_tool_rounds;

#[test]
fn file_config_parses_full_object() {
    let json = r#"{
        "engine": "toolcall",
        "model": "gpt-4o",
        "backend": "agent",
        "agent": "glmcc",
        "chunker": "token",
        "schema_mode": "evolving",
        "schema": "schema.json",
        "max_rounds": 3,
        "output": "mermaid"
    }"#;
    let cfg: FileConfig = serde_json::from_str(json).unwrap();
    assert!(matches!(cfg.engine, Some(Engine::Toolcall)));
    assert_eq!(cfg.model.as_deref(), Some("gpt-4o"));
    assert!(matches!(cfg.backend, Some(Backend::Agent)));
    assert_eq!(cfg.agent.as_deref(), Some("glmcc"));
    assert!(matches!(cfg.chunker, Some(Chunker::Token)));
    assert!(matches!(cfg.schema_mode, Some(SchemaModeArg::Evolving)));
    assert_eq!(cfg.schema.as_deref(), Some("schema.json"));
    assert_eq!(cfg.max_rounds, Some(3));
    assert!(matches!(cfg.output, Some(OutFmt::Mermaid)));
}

#[test]
fn file_config_parses_config_example_json() {
    // The shipped example must stay inside the real config surface:
    // FileConfig denies unknown fields, so a stale key here breaks every
    // user who copies the example.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/config.example.json");
    let body = std::fs::read_to_string(path).expect("config.example.json must exist");
    let cfg: FileConfig = serde_json::from_str(&body)
        .expect("config.example.json must parse as FileConfig (deny_unknown_fields)");
    assert!(matches!(cfg.engine, Some(Engine::Simple)));
    assert_eq!(cfg.max_concurrency, Some(8));
}

#[test]
fn file_config_partial_leaves_rest_none() {
    let cfg: FileConfig = serde_json::from_str(r#"{"engine": "schema-json"}"#).unwrap();
    assert!(matches!(cfg.engine, Some(Engine::SchemaJson)));
    assert!(cfg.model.is_none());
    assert!(cfg.backend.is_none());
    assert!(cfg.output.is_none());
}

#[test]
fn file_config_rejects_unknown_key() {
    let err = serde_json::from_str::<FileConfig>(r#"{"nope": 1}"#);
    assert!(err.is_err(), "unknown keys must be rejected");
}

#[test]
fn load_config_inline_json() {
    let cfg = load_config(Some(r#"{"engine": "toolcall", "max_rounds": 5}"#)).unwrap();
    assert!(matches!(cfg.engine, Some(Engine::Toolcall)));
    assert_eq!(cfg.max_rounds, Some(5));
}

#[test]
fn load_config_inline_json_with_leading_space() {
    let cfg = load_config(Some("   {\"output\": \"stats\"}")).unwrap();
    assert!(matches!(cfg.output, Some(OutFmt::Stats)));
}

#[test]
fn file_config_parses_ladybug_import_output() {
    let cfg: FileConfig = serde_json::from_str(r#"{"output": "ladybug-import"}"#).unwrap();
    assert!(matches!(cfg.output, Some(OutFmt::LadybugImport)));
}

#[test]
fn load_config_missing_explicit_path_errors() {
    let err = load_config(Some("/no/such/kg-extract-config-xyz.json"));
    assert!(err.is_err(), "an explicit missing path must error");
}

#[test]
fn parse_mock_tool_rounds_accepts_inline_single_round() {
    let rounds = parse_mock_tool_rounds(
        r#"[{"name":"add_entity","arguments":{"name":"AtlasDB","type":"PRODUCT"}}]"#,
    )
    .unwrap();
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].len(), 1);
    assert_eq!(rounds[0][0].name, "add_entity");
    assert_eq!(rounds[0][0].arguments["name"], "AtlasDB");
    assert_eq!(rounds[0][0].id, "mock_0_0");
}

#[test]
fn parse_mock_tool_rounds_accepts_file_multi_round() {
    let dir = std::env::temp_dir().join(format!("kg-tool-rounds-{}", nanoid::nanoid!()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rounds.json");
    std::fs::write(
        &path,
        r#"[
          [{"id":"e1","name":"add_entity","args":{"name":"A","type":"PRODUCT"}}],
          [{"name":"finish","arguments":{}}]
        ]"#,
    )
    .unwrap();

    let rounds = parse_mock_tool_rounds(path.to_str().unwrap()).unwrap();
    assert_eq!(rounds.len(), 2);
    assert_eq!(rounds[0][0].id, "e1");
    assert_eq!(rounds[0][0].arguments["name"], "A");
    assert_eq!(rounds[1][0].name, "finish");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn expand_tilde_expands_home() {
    let home = std::env::var("HOME").expect("HOME set in test env");
    assert_eq!(
        expand_tilde("~/foo/bar.json"),
        PathBuf::from(&home).join("foo/bar.json")
    );
    // No leading ~/ → passthrough.
    assert_eq!(
        expand_tilde("/abs/path.json"),
        PathBuf::from("/abs/path.json")
    );
    assert_eq!(
        expand_tilde("rel/path.json"),
        PathBuf::from("rel/path.json")
    );
}

#[test]
fn read_input_expands_tilde_for_file() {
    // `--file ~/x` must expand like `--config`, not fail with "No such file".
    let home = std::env::var("HOME").expect("HOME set in test env");
    let sub = format!("kg-extract-test-{}", nanoid::nanoid!());
    let dir = PathBuf::from(&home).join(&sub);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("in.txt"), "hello kg").unwrap();

    let got = read_input(&Some(format!("~/{sub}/in.txt"))).unwrap();
    assert_eq!(got, "hello kg");

    let _ = std::fs::remove_dir_all(&dir);
}

/// CLI flag beats config; config beats built-in default; default when neither set.
#[test]
fn precedence_cli_over_config_over_default() {
    let render = |argv: &[&str], cfg: FileConfig| {
        let m = Args::command().get_matches_from(argv);
        let args = Args::from_arg_matches(&m).unwrap();
        resolve(&m, &args, cfg)
    };

    // 1. CLI flag wins over config.
    let r = render(
        &["kg-extract", "--engine", "simple"],
        FileConfig {
            engine: Some(Engine::SchemaJson),
            ..Default::default()
        },
    );
    assert!(matches!(r.engine, Engine::Simple));

    // 2. Config wins when CLI flag absent.
    let r = render(
        &["kg-extract"],
        FileConfig {
            engine: Some(Engine::SchemaJson),
            chunker: Some(Chunker::Token),
            max_rounds: Some(4),
            schema_mode: Some(SchemaModeArg::Evolving),
            ..Default::default()
        },
    );
    assert!(matches!(r.engine, Engine::SchemaJson));
    assert!(matches!(r.chunker, Chunker::Token));
    assert_eq!(r.max_rounds, 4);
    assert!(matches!(r.schema_mode, SchemaModeArg::Evolving));

    // 3. Built-in default when neither sets it.
    let r = render(&["kg-extract"], FileConfig::default());
    assert!(matches!(r.engine, Engine::Simple));
    assert!(matches!(r.chunker, Chunker::Recursive));
    assert_eq!(r.max_rounds, 1);
    assert!(matches!(r.schema_mode, SchemaModeArg::Open));
    assert_eq!(r.agent, "minimaxcc");
}

#[test]
fn input_format_defaults_to_text_and_accepts_chunks() {
    let m = Args::command().get_matches_from(["kg-extract"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(matches!(args.input_format, InputFormat::Text));

    let m = Args::command().get_matches_from(["kg-extract", "--input-format", "chunks"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(matches!(args.input_format, InputFormat::Chunks));

    let m = Args::command().get_matches_from(["kg-extract", "-F", "chunks"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(matches!(args.input_format, InputFormat::Chunks));
}

#[test]
fn describe_and_dry_run_flags_parse() {
    let m = Args::command().get_matches_from(["kg-extract", "--describe", "--json"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(args.describe);
    assert!(args.json);

    let m = Args::command().get_matches_from(["kg-extract", "--dryrun", "--json"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(args.dry_run);
    assert!(args.json);

    let m = Args::command().get_matches_from(["kg-extract", "--dry-run"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(args.dry_run);
}

#[test]
fn describe_value_reports_json_and_dry_run_support() {
    let value = describe_value();
    assert_eq!(value["name"], "kg-extract");
    assert_eq!(value["supports"]["describe"], true);
    assert!(value["supports"]["json"]
        .as_str()
        .unwrap()
        .contains("--json"));
    assert!(value["supports"]["dry_run"]
        .as_str()
        .unwrap()
        .contains("plan"));
}

#[test]
fn dry_run_value_reports_no_input_or_backend_side_effects() {
    let m = Args::command().get_matches_from([
        "kg-extract",
        "--dry-run",
        "--json",
        "-e",
        "schema-json",
        "-b",
        "agent",
        "--agent",
        "glmcc",
        "-f",
        "doc.md",
        "-o",
        "stats",
    ]);
    let args = Args::from_arg_matches(&m).unwrap();
    let cfg = resolve(&m, &args, FileConfig::default());
    let value = dry_run_value(&args, &cfg, false);
    assert_eq!(value["dry_run"], true);
    assert_eq!(value["will_read_input"], false);
    assert_eq!(value["will_call_backend"], false);
    assert_eq!(value["input"]["source"], "doc.md");
    assert_eq!(value["config"]["engine"], "schema-json");
    assert_eq!(value["config"]["backend"], "agent");
    assert_eq!(value["config"]["agent"], "glmcc");
    assert_eq!(value["config"]["output"], "stats");
}

/// An explicit CLI value flag overrides a differing config value.
#[test]
fn precedence_cli_flag_overrides_config() {
    let m = Args::command().get_matches_from(["kg-extract", "--schema-mode", "fixed"]);
    let args = Args::from_arg_matches(&m).unwrap();
    let r = resolve(
        &m,
        &args,
        FileConfig {
            schema_mode: Some(SchemaModeArg::Evolving),
            ..Default::default()
        },
    );
    assert!(
        matches!(r.schema_mode, SchemaModeArg::Fixed),
        "explicit --schema-mode must win over config"
    );
}

// ---- print_response: every output-format arm must terminate Ok on a
// populated response. The branches are pure printing; this locks the dispatch
// wiring (no panic, no format mis-route) for all eight formats. ----

use kg_extract::types::{
    Entity, EntityType, ExtractionResponse, KnowledgeGraph, Predicate, PredicateType, Triple,
};

fn populated_response() -> ExtractionResponse {
    // One entity + one self-loop triple so the jsonl/mermaid/stats branches
    // have non-empty content to serialize.
    let widget = Entity::new("p", "Widget", EntityType::Product);
    let mut kg = KnowledgeGraph::new();
    kg.add_entity(widget.clone());
    kg.add_triple(Triple::new(
        widget.clone(),
        Predicate::with_label(PredicateType::Uses, "USES"),
        widget,
    ));
    ExtractionResponse::new(kg)
}

#[test]
fn print_response_json_round_trips_ok() {
    // stdout goes to the test capture; we only assert the arm returns Ok
    // (i.e. serialization of every format succeeds and dispatch doesn't panic).
    let r = populated_response();
    assert!(print_response(OutFmt::Json, &r, None).is_ok());
}

#[test]
fn print_response_all_formats_return_ok() {
    let r = populated_response();
    for fmt in [
        OutFmt::Json,
        OutFmt::Jsonl,
        OutFmt::KgProtocol,
        OutFmt::NodeLink,
        OutFmt::LadybugImport,
        OutFmt::Mermaid,
        OutFmt::Stats,
    ] {
        assert!(
            print_response(fmt, &r, None).is_ok(),
            "print_response must succeed for every output format"
        );
    }
}

#[cfg(feature = "community")]
#[test]
fn print_response_communities_returns_ok_with_feature() {
    let r = populated_response();
    assert!(print_response(OutFmt::Communities, &r, None).is_ok());
}

#[cfg(not(feature = "community"))]
#[test]
fn print_response_communities_bails_without_feature() {
    // The `communities` format parses without the feature (so configs stay
    // portable) but must refuse to run with an actionable error.
    let r = populated_response();
    let err = print_response(OutFmt::Communities, &r, None).unwrap_err();
    assert!(
        err.to_string().contains("--features community"),
        "error must name the missing feature, got: {err}"
    );
}

#[test]
fn communities_output_format_parses_from_cli_and_config() {
    let m = Args::command().get_matches_from(["kg-extract", "-o", "communities"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(matches!(args.output, OutFmt::Communities));

    let cfg: FileConfig = serde_json::from_str(r#"{"output": "communities"}"#).unwrap();
    assert!(matches!(cfg.output, Some(OutFmt::Communities)));
}

#[cfg(feature = "community-leiden")]
#[test]
fn print_response_communities_hierarchy_returns_ok_with_feature() {
    let r = populated_response();
    assert!(print_response(OutFmt::CommunitiesHierarchy, &r, None).is_ok());
}

#[cfg(not(feature = "community-leiden"))]
#[test]
fn print_response_communities_hierarchy_bails_without_feature() {
    // The `communities-hierarchy` format parses without the feature (so
    // configs stay portable) but must refuse to run with an actionable error.
    let r = populated_response();
    let err = print_response(OutFmt::CommunitiesHierarchy, &r, None).unwrap_err();
    assert!(
        err.to_string().contains("--features community-leiden"),
        "error must name the missing feature, got: {err}"
    );
}

#[test]
fn communities_hierarchy_output_format_parses_from_cli_and_config() {
    let m = Args::command().get_matches_from(["kg-extract", "-o", "communities-hierarchy"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(matches!(args.output, OutFmt::CommunitiesHierarchy));

    let cfg: FileConfig = serde_json::from_str(r#"{"output": "communities-hierarchy"}"#).unwrap();
    assert!(matches!(cfg.output, Some(OutFmt::CommunitiesHierarchy)));
}

#[test]
fn community_summaries_flag_parses_from_cli_and_config() {
    // Off by default.
    let m = Args::command().get_matches_from(["kg-extract"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(!args.community_summaries);

    let m = Args::command().get_matches_from(["kg-extract", "--community-summaries"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(args.community_summaries);

    // Presence-flag resolution: explicit CLI wins; otherwise the config value.
    let m = Args::command().get_matches_from(["kg-extract"]);
    let args = Args::from_arg_matches(&m).unwrap();
    let cfg: FileConfig = serde_json::from_str(r#"{"community_summaries": true}"#).unwrap();
    let resolved = resolve(&m, &args, cfg);
    assert!(resolved.community_summaries, "config file enables the flag");

    let m = Args::command().get_matches_from(["kg-extract", "--community-summaries"]);
    let args = Args::from_arg_matches(&m).unwrap();
    let resolved = resolve(&m, &args, FileConfig::default());
    assert!(resolved.community_summaries);
}

#[test]
fn canonical_direction_flag_parses_from_cli_and_config() {
    // Off by default.
    let m = Args::command().get_matches_from(["kg-extract"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(!args.canonical_direction);

    let m = Args::command().get_matches_from(["kg-extract", "--canonical-direction"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(args.canonical_direction);

    // Presence-flag resolution: explicit CLI wins; otherwise the config value.
    let m = Args::command().get_matches_from(["kg-extract"]);
    let args = Args::from_arg_matches(&m).unwrap();
    let cfg: FileConfig = serde_json::from_str(r#"{"canonical_direction": true}"#).unwrap();
    let resolved = resolve(&m, &args, cfg);
    assert!(resolved.canonical_direction, "config file enables the flag");

    let m = Args::command().get_matches_from(["kg-extract", "--canonical-direction"]);
    let args = Args::from_arg_matches(&m).unwrap();
    let resolved = resolve(&m, &args, FileConfig::default());
    assert!(resolved.canonical_direction);
}

#[cfg(feature = "community")]
#[test]
fn print_response_communities_accepts_precomputed_summaries() {
    // `--community-summaries` computes the document in main (async) and hands
    // it over; the print arm must accept it without recomputing detection.
    let r = populated_response();
    let precomputed = serde_json::json!({
        "num_communities": 1,
        "quality": null,
        "communities": {
            "0": {"members": ["p"], "name": "Widget Cluster", "summary": "All about widgets."}
        }
    });
    assert!(print_response(OutFmt::Communities, &r, Some(&precomputed)).is_ok());
}

#[cfg(feature = "community-leiden")]
#[test]
fn print_response_communities_hierarchy_accepts_precomputed_summaries() {
    let r = populated_response();
    let precomputed = serde_json::json!({
        "detector": "hierarchical-leiden",
        "num_levels": 1,
        "levels": [{
            "level": 0, "quality": 0.5, "num_communities": 1,
            "communities": {
                "0": {"members": ["p"], "name": "Widget Cluster", "summary": "All about widgets."}
            }
        }]
    });
    assert!(print_response(OutFmt::CommunitiesHierarchy, &r, Some(&precomputed)).is_ok());
}


// ---- kg.provider/v1 subcommands + cli_spec render equivalence ----

#[test]
fn provider_subcommands_parse() {
    let m = Args::command().get_matches_from(["kg-extract", "describe", "--json"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(matches!(
        args.command,
        Some(ProviderCommand::Describe { json: true })
    ));

    let m = Args::command().get_matches_from(["kg-extract", "available", "--json"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(matches!(
        args.command,
        Some(ProviderCommand::Available { json: true })
    ));

    let m = Args::command().get_matches_from([
        "kg-extract",
        "invoke",
        "extract.entities_relations",
        "--request",
        "req.json",
        "--artifacts-dir",
        "/tmp/x",
    ]);
    let args = Args::from_arg_matches(&m).unwrap();
    match args.command {
        Some(ProviderCommand::Invoke {
            capability_id,
            request,
            artifacts_dir,
        }) => {
            assert_eq!(capability_id, "extract.entities_relations");
            assert_eq!(request, "req.json");
            assert_eq!(artifacts_dir.as_deref(), Some("/tmp/x"));
        }
        other => panic!("expected invoke subcommand, got {other:?}"),
    }

    // --request defaults to '-' (stdin).
    let m = Args::command().get_matches_from(["kg-extract", "invoke", "detect.communities"]);
    let args = Args::from_arg_matches(&m).unwrap();
    match args.command {
        Some(ProviderCommand::Invoke { request, .. }) => assert_eq!(request, "-"),
        other => panic!("expected invoke subcommand, got {other:?}"),
    }

    // No subcommand → the classic extraction CLI still parses bare.
    let m = Args::command().get_matches_from(["kg-extract", "-e", "simple", "-b", "mock"]);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(args.command.is_none());
}

/// Render an argv from a cli_spec + request, following the acme convention:
/// `always ++ subcommand ++ positionals ++ flags(by order, tiebreak flag)`.
/// Booleans emit when true; other kinds emit `flag value` when the field is
/// present (`json` values are compact-encoded). This is the same walk the hub
/// performs, so equivalence here means the hub renders a working argv.
fn render_cli_spec(cli_spec: &serde_json::Value, request: &serde_json::Value) -> Vec<String> {
    let mut argv: Vec<String> = vec!["kg-extract".to_string()];
    let mut push_tokens = |key: &str| {
        if let Some(tokens) = cli_spec[key].as_array() {
            argv.extend(tokens.iter().filter_map(|t| t.as_str().map(str::to_string)));
        }
    };
    push_tokens("always");
    push_tokens("subcommand");
    push_tokens("positionals");

    let mut flags: Vec<&serde_json::Value> = cli_spec["flags"]
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    flags.sort_by(|a, b| {
        a["order"]
            .as_u64()
            .cmp(&b["order"].as_u64())
            .then_with(|| a["flag"].as_str().cmp(&b["flag"].as_str()))
    });
    for flag in flags {
        let name = flag["name"].as_str().unwrap();
        let token = flag["flag"].as_str().unwrap();
        let Some(value) = request.get(name) else {
            continue; // optional and absent → omitted
        };
        match flag["kind"].as_str().unwrap() {
            "boolean" => {
                if value.as_bool().unwrap_or(false) {
                    argv.push(token.to_string());
                }
            }
            "json" => {
                argv.push(token.to_string());
                argv.push(serde_json::to_string(value).unwrap());
            }
            _ => {
                argv.push(token.to_string());
                argv.push(match value {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                });
            }
        }
    }
    argv
}

#[test]
fn extract_cli_spec_renders_argv_equivalent_to_direct_cli() {
    let doc = kg_extract::provider::describe_document("0.0.0-test");
    let cap = &doc["capabilities"][0];
    assert_eq!(cap["capability_id"], "extract.entities_relations");

    let request = serde_json::json!({
        "text": "OpenAI developed GPT-4.",   // stdin-fed: no flag, no argv token
        "engine": "schema-json",
        "backend": "mock",
        "chunker": "char",
        "coref": true,
        "canonical_direction": false,          // false → flag omitted
        "max_rounds": 2,
        "mock_response": "{\"entities\":{},\"relationships\":[]}"
    });
    let argv = render_cli_spec(&cap["cli_spec"], &request);

    // `always` pins the kg-document artifact format first.
    assert_eq!(&argv[1..3], &["-o", "kg-protocol"]);
    let m = Args::command().get_matches_from(&argv);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(matches!(args.engine, Engine::SchemaJson));
    assert!(matches!(args.backend, Backend::Mock));
    assert!(matches!(args.chunker, Chunker::Char));
    assert!(args.coref);
    assert!(!args.canonical_direction);
    assert_eq!(args.max_rounds, 2);
    assert_eq!(
        args.mock_response.as_deref(),
        Some("{\"entities\":{},\"relationships\":[]}")
    );
    assert!(matches!(args.output, OutFmt::KgProtocol));
    assert!(args.command.is_none(), "extract renders the flat-flag form");
}

#[test]
fn extract_cli_spec_renders_full_surface_without_clap_errors() {
    // Every cli_spec flag set at once must parse — this locks flag tokens
    // (`--schema-mode` etc.) against the real clap surface.
    let doc = kg_extract::provider::describe_document("0.0.0-test");
    let cap = &doc["capabilities"][0];
    let request = serde_json::json!({
        "file": "doc.txt",
        "input_format": "chunks",
        "engine": "toolcall",
        "backend": "agent",
        "agent": "glmcc",
        "model": "glm-5.1",
        "chunker": "token",
        "schema": "schema.json",
        "schema_mode": "evolving",
        "preset": "general/concept_graph",
        "preset_file": "tpl.yaml",
        "lang": "zh",
        "max_rounds": 3,
        "merge_strategy": "field-union",
        "coref": true,
        "canonical_direction": true,
        "max_concurrency": 4,
        "relation_gleaning": 2,
        "mock_response": "canned",
        "mock_tool_calls": [{"name": "finish", "arguments": {}}]
    });
    let argv = render_cli_spec(&cap["cli_spec"], &request);
    let m = Args::command().get_matches_from(&argv);
    let args = Args::from_arg_matches(&m).unwrap();
    assert!(matches!(args.engine, Engine::Toolcall));
    assert!(matches!(args.schema_mode, SchemaModeArg::Evolving));
    assert!(matches!(args.merge_strategy, MergeStrategyArg::FieldUnion));
    assert!(matches!(args.input_format, InputFormat::Chunks));
    assert_eq!(args.agent, "glmcc");
    assert_eq!(args.lang.as_deref(), Some("zh"));
    assert_eq!(args.relation_gleaning, 2);
}

#[test]
fn graph_in_cli_specs_render_invoke_argv_that_parses() {
    let doc = kg_extract::provider::describe_document("0.0.0-test");
    for cap in doc["capabilities"].as_array().unwrap()[1..].iter() {
        let id = cap["capability_id"].as_str().unwrap();
        let argv = render_cli_spec(&cap["cli_spec"], &serde_json::json!({}));
        // ["kg-extract", "invoke", <id>, "--request", "-"]: subcommand tokens
        // come from cli_spec, --request - is the rendered default... but the
        // renderer only emits flags present in the request, so defaults that
        // match clap's own default are safe to omit.
        assert_eq!(&argv[1..3], &["invoke", id]);
        let m = Args::command().get_matches_from(&argv);
        let args = Args::from_arg_matches(&m).unwrap();
        match args.command {
            Some(ProviderCommand::Invoke {
                capability_id,
                request,
                ..
            }) => {
                assert_eq!(capability_id, id);
                assert_eq!(request, "-");
            }
            other => panic!("{id}: expected invoke subcommand, got {other:?}"),
        }
    }
}

#[test]
fn legacy_describe_points_at_provider_protocol() {
    let value = describe_value();
    assert!(value["supports"]["provider_protocol"]
        .as_str()
        .unwrap()
        .contains("kg.provider/v1"));
}
