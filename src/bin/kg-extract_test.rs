use super::*;

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
fn load_config_missing_explicit_path_errors() {
    let err = load_config(Some("/no/such/kg-extract-config-xyz.json"));
    assert!(err.is_err(), "an explicit missing path must error");
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
