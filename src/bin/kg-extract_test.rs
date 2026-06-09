use super::*;

#[test]
fn file_config_parses_full_object() {
    let json = r#"{
        "engine": "triplex",
        "model": "gpt-4o",
        "backend": "agent",
        "agent": "glmcc",
        "chunker": "token",
        "youtu_agent": true,
        "community": true,
        "toolcall_agent": false,
        "max_rounds": 3,
        "output": "mermaid"
    }"#;
    let cfg: FileConfig = serde_json::from_str(json).unwrap();
    assert!(matches!(cfg.engine, Some(Engine::Triplex)));
    assert_eq!(cfg.model.as_deref(), Some("gpt-4o"));
    assert!(matches!(cfg.backend, Some(Backend::Agent)));
    assert_eq!(cfg.agent.as_deref(), Some("glmcc"));
    assert!(matches!(cfg.chunker, Some(Chunker::Token)));
    assert_eq!(cfg.youtu_agent, Some(true));
    assert_eq!(cfg.community, Some(true));
    assert_eq!(cfg.toolcall_agent, Some(false));
    assert_eq!(cfg.max_rounds, Some(3));
    assert!(matches!(cfg.output, Some(OutFmt::Mermaid)));
}

#[test]
fn file_config_partial_leaves_rest_none() {
    let cfg: FileConfig = serde_json::from_str(r#"{"engine": "youtu"}"#).unwrap();
    assert!(matches!(cfg.engine, Some(Engine::Youtu)));
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
    assert_eq!(expand_tilde("~/foo/bar.json"), PathBuf::from(&home).join("foo/bar.json"));
    // No leading ~/ → passthrough.
    assert_eq!(expand_tilde("/abs/path.json"), PathBuf::from("/abs/path.json"));
    assert_eq!(expand_tilde("rel/path.json"), PathBuf::from("rel/path.json"));
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
    let r = render(&["kg-extract", "--engine", "simple"], FileConfig {
        engine: Some(Engine::Youtu),
        ..Default::default()
    });
    assert!(matches!(r.engine, Engine::Simple));

    // 2. Config wins when CLI flag absent.
    let r = render(&["kg-extract"], FileConfig {
        engine: Some(Engine::Youtu),
        chunker: Some(Chunker::Token),
        max_rounds: Some(4),
        youtu_agent: Some(true),
        ..Default::default()
    });
    assert!(matches!(r.engine, Engine::Youtu));
    assert!(matches!(r.chunker, Chunker::Token));
    assert_eq!(r.max_rounds, 4);
    assert!(r.youtu_agent);

    // 3. Built-in default when neither sets it.
    let r = render(&["kg-extract"], FileConfig::default());
    assert!(matches!(r.engine, Engine::Simple));
    assert!(matches!(r.chunker, Chunker::Recursive));
    assert_eq!(r.max_rounds, 1);
    assert!(!r.youtu_agent);
    assert_eq!(r.agent, "minimaxcc");
}

/// A bool flag explicitly passed must override a config value of `false`.
#[test]
fn precedence_bool_flag_overrides_config_false() {
    let m = Args::command().get_matches_from(["kg-extract", "--community"]);
    let args = Args::from_arg_matches(&m).unwrap();
    let r = resolve(&m, &args, FileConfig { community: Some(false), ..Default::default() });
    assert!(r.community, "explicit --community must win over config false");
}
