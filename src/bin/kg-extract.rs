//! CLI for kg-extract: extract a knowledge graph from text and emit JSON or
//! Mermaid. Mirrors `python -m graph.kg_extractor`.
//!
//! Settings can come from three places, highest precedence first:
//!   1. an explicit command-line flag,
//!   2. a config file (`--config`, or `~/.kg-extract/config.json` by default),
//!   3. the built-in default.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::parser::ValueSource;
use clap::{ArgMatches, CommandFactory, FromArgMatches, Parser, ValueEnum};
use serde::Deserialize;

use kg_extract::backend::{LlmBackend, MockBackend, PiAgentBackend, SdkAgentBackend};
use kg_extract::extractor::{
    AgenticExtractor, Extractor, SchemaMode, SimpleExtractor, ToolCallExtractor, SchemaJsonExtractor,
};
use kg_extract::template::{gallery, TemplateCfg};
use kg_extract::types::{ChunkStrategy, MergeStrategy, Schema};

#[derive(Copy, Clone, Debug, ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Engine {
    Simple,
    SchemaJson,
    Toolcall,
    /// Experimental: whole document through one sandboxed, multi-turn SDK
    /// session (slices fed as turns; agent can grep/read the doc for context).
    Agentic,
}

#[derive(Copy, Clone, Debug, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Backend {
    /// In-process `llms` crate (requires the `llms-backend` feature).
    Llms,
    /// Agent CLI driven through the `claude-agent-sdk-rs` stream-json protocol
    /// (minimaxcc / glmcc / mimocc), or pi-rs's `pi-agent`. Provider chosen by
    /// --agent.
    Agent,
    /// Deterministic mock (reads a canned response from --mock-response).
    Mock,
}

#[derive(Copy, Clone, Debug, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Chunker {
    Char,
    Recursive,
    Token,
}

impl From<Chunker> for ChunkStrategy {
    fn from(c: Chunker) -> Self {
        match c {
            Chunker::Char => ChunkStrategy::Char,
            Chunker::Recursive => ChunkStrategy::Recursive,
            Chunker::Token => ChunkStrategy::Token,
        }
    }
}

/// Schema mode (CLI mirror of [`SchemaMode`]).
#[derive(Copy, Clone, Debug, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SchemaModeArg {
    Open,
    Fixed,
    Evolving,
}

impl From<SchemaModeArg> for SchemaMode {
    fn from(m: SchemaModeArg) -> Self {
        match m {
            SchemaModeArg::Open => SchemaMode::Open,
            SchemaModeArg::Fixed => SchemaMode::Fixed,
            SchemaModeArg::Evolving => SchemaMode::Evolving,
        }
    }
}

/// How duplicate entities are combined (CLI mirror of [`MergeStrategy`]).
#[derive(Copy, Clone, Debug, ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum MergeStrategyArg {
    KeepExisting,
    KeepIncoming,
    FieldUnion,
    Llm,
}

impl From<MergeStrategyArg> for MergeStrategy {
    fn from(s: MergeStrategyArg) -> Self {
        match s {
            MergeStrategyArg::KeepExisting => MergeStrategy::KeepExisting,
            MergeStrategyArg::KeepIncoming => MergeStrategy::KeepIncoming,
            MergeStrategyArg::FieldUnion => MergeStrategy::FieldUnion,
            MergeStrategyArg::Llm => MergeStrategy::Llm,
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
enum OutFmt {
    Json,
    #[serde(rename = "node-link")]
    #[value(name = "node-link", alias = "nodelink")]
    NodeLink,
    Mermaid,
    Stats,
}

/// Defaults loaded from a config file (`--config` or `~/.kg-extract/config.json`).
///
/// Every field is optional: a missing field falls back to the CLI's built-in
/// default. An explicit command-line flag always wins over the config file.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    engine: Option<Engine>,
    model: Option<String>,
    backend: Option<Backend>,
    agent: Option<String>,
    chunker: Option<Chunker>,
    schema_mode: Option<SchemaModeArg>,
    schema: Option<String>,
    preset: Option<String>,
    preset_file: Option<String>,
    lang: Option<String>,
    max_rounds: Option<usize>,
    merge_strategy: Option<MergeStrategyArg>,
    max_concurrency: Option<usize>,
    relation_gleaning: Option<usize>,
    output: Option<OutFmt>,
}

/// Extract a knowledge graph from text.
#[derive(Parser, Debug)]
#[command(name = "kg-extract", version, about)]
struct Args {
    /// Config file path, or an inline JSON object (a value starting with '{').
    /// Defaults to ~/.kg-extract/config.json when present.
    #[arg(short = 'c', long)]
    config: Option<String>,

    /// Extraction engine.
    #[arg(short, long, value_enum, default_value_t = Engine::Simple)]
    engine: Engine,

    /// Input file (use '-' or omit for stdin).
    #[arg(short, long)]
    file: Option<String>,

    /// Model name (overrides the engine default).
    #[arg(short, long)]
    model: Option<String>,

    /// Completion backend.
    #[arg(short, long, value_enum, default_value_t = Backend::Llms)]
    backend: Backend,

    /// Agent CLI to use when --backend agent: minimaxcc / glmcc / mimocc /
    /// pi-agent (default minimaxcc).
    #[arg(long, default_value = "minimaxcc")]
    agent: String,

    /// Chunking strategy.
    #[arg(short = 'k', long, value_enum, default_value_t = Chunker::Recursive)]
    chunker: Chunker,

    /// Schema mode for the schema-driven engines (schema-json / toolcall): `open` (no
    /// predefined types, default) / `fixed` (use only the schema) / `evolving`
    /// (seed schema, allow new types). `fixed` and `evolving` require --schema.
    #[arg(long, value_enum, default_value_t = SchemaModeArg::Open)]
    schema_mode: SchemaModeArg,

    /// Schema JSON file (entity/relation/attribute types) for schema-json / toolcall.
    /// Required for --schema-mode fixed|evolving; ignored by --schema-mode open.
    #[arg(long)]
    schema: Option<String>,

    /// Built-in extraction preset (rich template) by name, e.g.
    /// `general/concept_graph`, `finance/event_timeline`, or a bare `graph`
    /// (resolved under `general/`). Routes through the schema-json engine and
    /// drives the prompt from the preset's guideline + fields. See --list-presets.
    #[arg(long)]
    preset: Option<String>,

    /// Path to your own template YAML (same format as the built-in presets).
    /// Takes precedence over --preset.
    #[arg(long)]
    preset_file: Option<String>,

    /// Language to render the preset/template in (e.g. `zh`, `en`). Defaults to
    /// the template's first declared language.
    #[arg(long)]
    lang: Option<String>,

    /// List the bundled presets (key + description) and exit.
    #[arg(long)]
    list_presets: bool,

    /// Tool-call engine: max tool-calling rounds (1 = single-round collection).
    #[arg(long, default_value_t = 1)]
    max_rounds: usize,

    /// How duplicate entities are combined when deduping: keep-existing
    /// (default) / keep-incoming / field-union / llm (LLM-synthesised description).
    #[arg(long, value_enum, default_value_t = MergeStrategyArg::KeepExisting)]
    merge_strategy: MergeStrategyArg,

    /// Max segments extracted concurrently (Simple engine). 1 = sequential.
    #[arg(long, default_value_t = 8)]
    max_concurrency: usize,

    /// Simple engine: targeted relation-gleaning rounds run after entity
    /// gleaning. Each round re-questions orphan entities (no relationship) to
    /// recover their edges. 0 (default) keeps the original behaviour.
    #[arg(long, default_value_t = 0)]
    relation_gleaning: usize,

    /// Canned response for --backend mock.
    #[arg(long)]
    mock_response: Option<String>,

    /// Output format.
    #[arg(short, long, value_enum, default_value_t = OutFmt::Json)]
    output: OutFmt,
}

/// Resolved settings after merging CLI flags over the config file.
struct Resolved {
    engine: Engine,
    model: Option<String>,
    backend: Backend,
    agent: String,
    chunker: Chunker,
    schema_mode: SchemaModeArg,
    schema: Option<String>,
    preset: Option<String>,
    preset_file: Option<String>,
    lang: Option<String>,
    max_rounds: usize,
    merge_strategy: MergeStrategyArg,
    max_concurrency: usize,
    relation_gleaning: usize,
    output: OutFmt,
}

fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

fn default_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".kg-extract").join("config.json"))
}

/// Load the config from `--config` (inline JSON or a path) or, when absent, the
/// default `~/.kg-extract/config.json` if it exists. A missing default file is
/// fine (empty config); a missing *explicit* path or bad JSON is an error.
fn load_config(arg: Option<&str>) -> anyhow::Result<FileConfig> {
    match arg {
        Some(s) if s.trim_start().starts_with('{') => {
            serde_json::from_str(s).context("parsing inline --config JSON")
        }
        Some(path) => {
            let path = expand_tilde(path);
            let body = std::fs::read_to_string(&path)
                .with_context(|| format!("reading config file {}", path.display()))?;
            serde_json::from_str(&body)
                .with_context(|| format!("parsing config file {}", path.display()))
        }
        None => match default_config_path() {
            Some(p) if p.exists() => {
                let body = std::fs::read_to_string(&p)
                    .with_context(|| format!("reading config file {}", p.display()))?;
                serde_json::from_str(&body)
                    .with_context(|| format!("parsing config file {}", p.display()))
            }
            _ => Ok(FileConfig::default()),
        },
    }
}

/// Pick a value with precedence: explicit CLI flag > config file > built-in
/// default. `cli` already carries the built-in default when the flag is absent,
/// so we only prefer the config value when the flag was *not* on the command
/// line. Works for value options and `bool` flags alike.
fn pick<T: Clone>(m: &ArgMatches, id: &str, cli: T, cfg: Option<T>) -> T {
    if m.value_source(id) == Some(ValueSource::CommandLine) {
        cli
    } else {
        cfg.unwrap_or(cli)
    }
}

fn resolve(m: &ArgMatches, args: &Args, cfg: FileConfig) -> Resolved {
    // `model` has no built-in default, so resolution is: CLI > config > None.
    let model = if m.value_source("model") == Some(ValueSource::CommandLine) {
        args.model.clone()
    } else {
        cfg.model.clone().or_else(|| args.model.clone())
    };
    Resolved {
        engine: pick(m, "engine", args.engine, cfg.engine),
        model,
        backend: pick(m, "backend", args.backend, cfg.backend),
        agent: pick(m, "agent", args.agent.clone(), cfg.agent),
        chunker: pick(m, "chunker", args.chunker, cfg.chunker),
        schema_mode: pick(m, "schema_mode", args.schema_mode, cfg.schema_mode),
        // `schema` has no built-in default (like `model`): CLI > config > None.
        schema: if m.value_source("schema") == Some(ValueSource::CommandLine) {
            args.schema.clone()
        } else {
            cfg.schema.clone().or_else(|| args.schema.clone())
        },
        // preset / preset_file / lang have no built-in default: CLI > config > None.
        preset: if m.value_source("preset") == Some(ValueSource::CommandLine) {
            args.preset.clone()
        } else {
            cfg.preset.clone().or_else(|| args.preset.clone())
        },
        preset_file: if m.value_source("preset_file") == Some(ValueSource::CommandLine) {
            args.preset_file.clone()
        } else {
            cfg.preset_file.clone().or_else(|| args.preset_file.clone())
        },
        lang: if m.value_source("lang") == Some(ValueSource::CommandLine) {
            args.lang.clone()
        } else {
            cfg.lang.clone().or_else(|| args.lang.clone())
        },
        max_rounds: pick(m, "max_rounds", args.max_rounds, cfg.max_rounds),
        merge_strategy: pick(m, "merge_strategy", args.merge_strategy, cfg.merge_strategy),
        max_concurrency: pick(m, "max_concurrency", args.max_concurrency, cfg.max_concurrency),
        relation_gleaning: pick(m, "relation_gleaning", args.relation_gleaning, cfg.relation_gleaning),
        output: pick(m, "output", args.output, cfg.output),
    }
}

fn read_input(file: &Option<String>) -> anyhow::Result<String> {
    match file.as_deref() {
        None | Some("-") => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            Ok(s)
        }
        Some(path) => {
            let path = expand_tilde(path);
            std::fs::read_to_string(&path)
                .with_context(|| format!("reading input file {}", path.display()))
        }
    }
}

fn make_backend(
    backend: Backend,
    agent: &str,
    mock_response: Option<&str>,
) -> anyhow::Result<Arc<dyn LlmBackend>> {
    match backend {
        Backend::Agent => {
            // pi-agent (from pi-rs) has a different CLI contract than the
            // Claude-Code wrappers, so it gets its own backend. Everything else
            // is driven through the structured stream-json SDK.
            if PiAgentBackend::accepts(agent) {
                return Ok(Arc::new(PiAgentBackend::new()));
            }
            Ok(Arc::new(SdkAgentBackend::for_agent(agent)?))
        }
        Backend::Mock => {
            let resp = mock_response.unwrap_or_default().to_string();
            Ok(Arc::new(MockBackend::single(resp)))
        }
        Backend::Llms => {
            #[cfg(feature = "llms-backend")]
            {
                Ok(Arc::new(kg_extract::backend::LlmsBackend::new()))
            }
            #[cfg(not(feature = "llms-backend"))]
            {
                anyhow::bail!(
                    "the `llms` backend requires building with --features llms-backend; \
                     use --backend agent or --backend mock instead"
                )
            }
        }
    }
}

/// Resolve a rich template from `--preset-file` (a user YAML, takes precedence)
/// or `--preset` (a bundled preset key). Returns `None` when neither is set.
fn load_template(cfg: &Resolved) -> anyhow::Result<Option<TemplateCfg>> {
    if let Some(path) = &cfg.preset_file {
        let tpl = TemplateCfg::from_yaml_file(expand_tilde(path))?;
        return Ok(Some(tpl));
    }
    if let Some(name) = &cfg.preset {
        let tpl = gallery::get(name).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown preset '{name}'. Run with --list-presets to see the {} available.",
                gallery::list().len()
            )
        })?;
        return Ok(Some(tpl));
    }
    Ok(None)
}

/// Print the bundled presets (`key  [type]  description`) to stdout.
fn print_presets() {
    for p in gallery::list() {
        let lang = p.template.language.first();
        println!(
            "{:<34} [{}]  {}",
            p.key,
            p.template.autotype.as_str(),
            p.template.describe(&lang)
        );
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let matches = Args::command().get_matches();
    let args = Args::from_arg_matches(&matches)?;

    if args.list_presets {
        print_presets();
        return Ok(());
    }

    let file_cfg = load_config(args.config.as_deref())?;
    let mut cfg = resolve(&matches, &args, file_cfg);

    // A preset/template (rich, prompt-driving) is only honoured by the schema-json
    // engine, which emits the JSON contract it produces. Load it and route there.
    let template = load_template(&cfg)?;
    if template.is_some() && !matches!(cfg.engine, Engine::SchemaJson) {
        eprintln!("note: --preset/--preset-file routes through the schema-json engine");
        cfg.engine = Engine::SchemaJson;
    }

    let text = read_input(&args.file)?;
    if text.trim().is_empty() {
        anyhow::bail!("no input text (provide --file or pipe via stdin)");
    }
    // The agentic engine drives the SDK client itself (cwd sandbox + one
    // long-lived session), so it bypasses `make_backend` and its `--backend`
    // value — the provider is chosen by `--agent`.
    let response = if matches!(cfg.engine, Engine::Agentic) {
        let mut c = AgenticExtractor::default_config();
        c.chunker = cfg.chunker.into();
        if let Some(m) = &cfg.model {
            c.model_name = m.clone();
        }
        // Fixed mode validates each slice against this schema and drops
        // out-of-schema records; Open/Evolving leave it as hints.
        if let Some(path) = &cfg.schema {
            c.spec.schema = Schema::from_json_file(expand_tilde(path))
                .with_context(|| format!("loading --schema {path}"))?;
        }
        AgenticExtractor::with_config(&cfg.agent, c)
            .schema_mode(cfg.schema_mode.into())
            .relation_gleanings(cfg.relation_gleaning)
            .extract(&text)
            .await?
    } else {
        let backend = make_backend(cfg.backend, &cfg.agent, args.mock_response.as_deref())?;
        match cfg.engine {
        Engine::Simple => {
            let mut c = SimpleExtractor::default_config();
            c.chunker = cfg.chunker.into();
            c.max_concurrency = cfg.max_concurrency;
            c.spec.merge_strategy = cfg.merge_strategy.into();
            if let Some(m) = &cfg.model {
                c.model_name = m.clone();
            }
            SimpleExtractor::with_config(backend, c)
                .relation_gleanings(cfg.relation_gleaning)
                .extract(&text)
                .await?
        }
        Engine::SchemaJson => {
            let mut c = SchemaJsonExtractor::default_config();
            c.chunker = cfg.chunker.into();
            c.spec.merge_strategy = cfg.merge_strategy.into();
            if let Some(m) = &cfg.model {
                c.model_name = m.clone();
            }
            if let Some(path) = &cfg.schema {
                c.spec.schema = Schema::from_json_file(expand_tilde(path))
                    .with_context(|| format!("loading --schema {path}"))?;
            }
            if let Some(tpl) = template {
                c.spec.language = cfg.lang.clone();
                c.spec.template = Some(tpl);
            }
            SchemaJsonExtractor::with_config(backend, c)
                .schema_mode(cfg.schema_mode.into())
                .extract(&text)
                .await?
        }
        Engine::Toolcall => {
            let mut c = ToolCallExtractor::default_config();
            c.chunker = cfg.chunker.into();
            c.spec.merge_strategy = cfg.merge_strategy.into();
            if let Some(m) = &cfg.model {
                c.model_name = m.clone();
            }
            if let Some(path) = &cfg.schema {
                c.spec.schema = Schema::from_json_file(expand_tilde(path))
                    .with_context(|| format!("loading --schema {path}"))?;
            }
            ToolCallExtractor::with_config(backend, c)
                .schema_mode(cfg.schema_mode.into())
                .max_rounds(cfg.max_rounds)
                .extract(&text)
                .await?
        }
            Engine::Agentic => unreachable!("agentic handled above"),
        }
    };

    match cfg.output {
        OutFmt::Json => {
            println!("{}", serde_json::to_string_pretty(&response.knowledge_graph.to_dict())?)
        }
        OutFmt::NodeLink => {
            println!("{}", serde_json::to_string_pretty(&response.knowledge_graph.to_node_link())?)
        }
        OutFmt::Mermaid => println!("{}", response.get_mermaid_code()),
        OutFmt::Stats => println!("{}", serde_json::to_string_pretty(&response.get_stats())?),
    }
    Ok(())
}

#[cfg(test)]
#[path = "kg-extract_test.rs"]
mod tests;
