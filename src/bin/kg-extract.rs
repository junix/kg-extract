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
use serde_json::json;

use kg_extract::backend::{
    LlmBackend, MockBackend, PiAgentBackend, SdkAgentBackend, ToolInvocation,
};
use kg_extract::extractor::{
    AgenticExtractor, Extractor, SchemaJsonExtractor, SchemaMode, SimpleExtractor,
    ToolCallExtractor,
};
use kg_extract::template::{gallery, TemplateCfg};
use kg_extract::types::{ChunkStrategy, CorefMode, ExtractionResponse, MergeStrategy, Schema};

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

/// What the input stream/file contains.
#[derive(Copy, Clone, Debug, ValueEnum)]
enum InputFormat {
    /// Plain text — segmented internally per --chunker.
    Text,
    /// Pre-chunked chonkie output (JSON array or JSONL, e.g. `chonkie --jsonl`).
    /// The chunking engines (simple / agentic) use the chunks AS-IS instead of
    /// re-chunking; the single-shot engines (schema-json / toolcall) join them.
    Chunks,
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
    Jsonl,
    #[serde(rename = "kg-protocol")]
    #[value(name = "kg-protocol", alias = "kg")]
    KgProtocol,
    #[serde(rename = "node-link")]
    #[value(name = "node-link", alias = "nodelink")]
    NodeLink,
    #[serde(rename = "ladybug-import")]
    #[value(name = "ladybug-import", alias = "lbug-import")]
    LadybugImport,
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
    coref: Option<bool>,
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

    /// Input format: plain `text` (default), or chonkie `chunks` (JSON/JSONL,
    /// e.g. from `chonkie --jsonl`) consumed as-is — the chunking engines skip
    /// their internal re-chunking and extract per given chunk.
    #[arg(short = 'F', long, value_enum, default_value_t = InputFormat::Text)]
    input_format: InputFormat,

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

    /// Describe CLI capabilities and output contracts, then exit.
    #[arg(long)]
    describe: bool,

    /// Print the resolved extraction plan without reading input or calling a backend.
    #[arg(long = "dry-run", alias = "dryrun")]
    dry_run: bool,

    /// Tool-call engine: max tool-calling rounds (1 = single-round collection).
    #[arg(long, default_value_t = 1)]
    max_rounds: usize,

    /// How duplicate entities are combined when deduping: keep-existing
    /// (default) / keep-incoming / field-union / llm (LLM-synthesised description).
    #[arg(long, value_enum, default_value_t = MergeStrategyArg::KeepExisting)]
    merge_strategy: MergeStrategyArg,

    /// Entity coreference: also merge surface variants of the same name (case,
    /// punctuation, corporate suffixes, near-identical spellings) across chunks,
    /// not just exact-label duplicates. Off by default.
    #[arg(long)]
    coref: bool,

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

    /// Scripted tool calls for --backend mock with --engine toolcall.
    /// Accepts inline JSON or a JSON file path. Shape: `[{"name": "...",
    /// "arguments": {...}}]` for one round, or `[[...], [...]]` for many rounds.
    #[arg(long)]
    mock_tool_calls: Option<String>,

    /// Output format.
    #[arg(short, long, value_enum, default_value_t = OutFmt::Json)]
    output: OutFmt,

    /// Machine-readable JSON for --describe and --dry-run only.
    #[arg(long)]
    json: bool,
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
    coref: bool,
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
        // A presence flag: explicit `--coref` wins, else the config file value.
        coref: args.coref || cfg.coref.unwrap_or(false),
        max_concurrency: pick(
            m,
            "max_concurrency",
            args.max_concurrency,
            cfg.max_concurrency,
        ),
        relation_gleaning: pick(
            m,
            "relation_gleaning",
            args.relation_gleaning,
            cfg.relation_gleaning,
        ),
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
    mock_tool_calls: Option<&str>,
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
            let mock = MockBackend::single(resp);
            if let Some(tool_calls) = mock_tool_calls {
                Ok(Arc::new(
                    mock.with_tool_rounds(parse_mock_tool_rounds(tool_calls)?),
                ))
            } else {
                Ok(Arc::new(mock))
            }
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

#[derive(Deserialize)]
struct RawToolCall {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default, alias = "args")]
    arguments: serde_json::Value,
}

fn parse_mock_tool_rounds(input: &str) -> anyhow::Result<Vec<Vec<ToolInvocation>>> {
    let raw = if input.trim_start().starts_with('[') {
        input.to_string()
    } else {
        let path = expand_tilde(input);
        std::fs::read_to_string(&path)
            .with_context(|| format!("reading --mock-tool-calls {}", path.display()))?
    };
    let value: serde_json::Value =
        serde_json::from_str(&raw).context("parsing --mock-tool-calls JSON")?;
    let rounds = if value
        .as_array()
        .and_then(|a| a.first())
        .is_some_and(|first| first.is_array())
    {
        serde_json::from_value::<Vec<Vec<RawToolCall>>>(value)?
            .into_iter()
            .enumerate()
            .map(|(round_idx, round)| raw_round_to_invocations(round_idx, round))
            .collect()
    } else {
        vec![raw_round_to_invocations(
            0,
            serde_json::from_value::<Vec<RawToolCall>>(value)?,
        )]
    };
    Ok(rounds)
}

fn raw_round_to_invocations(round_idx: usize, calls: Vec<RawToolCall>) -> Vec<ToolInvocation> {
    calls
        .into_iter()
        .enumerate()
        .map(|(call_idx, call)| ToolInvocation {
            id: call
                .id
                .unwrap_or_else(|| format!("mock_{round_idx}_{call_idx}")),
            name: call.name,
            arguments: call.arguments,
        })
        .collect()
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

fn engine_name(v: Engine) -> &'static str {
    match v {
        Engine::Simple => "simple",
        Engine::SchemaJson => "schema-json",
        Engine::Toolcall => "toolcall",
        Engine::Agentic => "agentic",
    }
}

fn backend_name(v: Backend) -> &'static str {
    match v {
        Backend::Llms => "llms",
        Backend::Agent => "agent",
        Backend::Mock => "mock",
    }
}

fn chunker_name(v: Chunker) -> &'static str {
    match v {
        Chunker::Char => "char",
        Chunker::Recursive => "recursive",
        Chunker::Token => "token",
    }
}

fn input_format_name(v: InputFormat) -> &'static str {
    match v {
        InputFormat::Text => "text",
        InputFormat::Chunks => "chunks",
    }
}

fn schema_mode_name(v: SchemaModeArg) -> &'static str {
    match v {
        SchemaModeArg::Open => "open",
        SchemaModeArg::Fixed => "fixed",
        SchemaModeArg::Evolving => "evolving",
    }
}

fn merge_strategy_name(v: MergeStrategyArg) -> &'static str {
    match v {
        MergeStrategyArg::KeepExisting => "keep-existing",
        MergeStrategyArg::KeepIncoming => "keep-incoming",
        MergeStrategyArg::FieldUnion => "field-union",
        MergeStrategyArg::Llm => "llm",
    }
}

fn output_name(v: OutFmt) -> &'static str {
    match v {
        OutFmt::Json => "json",
        OutFmt::Jsonl => "jsonl",
        OutFmt::KgProtocol => "kg-protocol",
        OutFmt::NodeLink => "node-link",
        OutFmt::LadybugImport => "ladybug-import",
        OutFmt::Mermaid => "mermaid",
        OutFmt::Stats => "stats",
    }
}

fn describe_value() -> serde_json::Value {
    json!({
        "name": "kg-extract",
        "summary": "Extract a knowledge graph from text with simple, schema-json, toolcall, or agentic engines.",
        "supports": {
            "describe": true,
            "json": "use --json with --describe or --dry-run; extraction JSON is selected with -o json/jsonl/kg-protocol/node-link/ladybug-import/stats",
            "dry_run": "prints the resolved extraction plan without reading input or calling a backend"
        },
        "examples": [
            "kg-extract --describe --json",
            "kg-extract --dry-run --json -e schema-json -b agent --agent minimaxcc -f doc.md",
            "kg-extract -e simple -b mock --mock-response '{\"entities\":{},\"relationships\":[]}' -f doc.txt -o json"
        ],
        "outputs": [
            "json",
            "jsonl",
            "kg-protocol",
            "node-link",
            "ladybug-import",
            "mermaid",
            "stats"
        ]
    })
}

fn print_describe(as_json: bool) -> anyhow::Result<()> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(&describe_value())?);
    } else {
        println!("kg-extract");
        println!("  Extract a knowledge graph from text.");
        println!();
        println!("Supports:");
        println!("  --describe        show CLI capabilities");
        println!("  --dry-run         print the resolved plan without reading input or calling a backend");
        println!("  --json            machine-readable describe/dry-run output");
        println!("  -o json|jsonl|... extraction output format");
    }
    Ok(())
}

fn dry_run_value(args: &Args, cfg: &Resolved, template_loaded: bool) -> serde_json::Value {
    json!({
        "dry_run": true,
        "will_read_input": false,
        "will_call_backend": false,
        "input": {
            "source": args.file.as_deref().unwrap_or("stdin"),
            "format": input_format_name(args.input_format)
        },
        "config": {
            "engine": engine_name(cfg.engine),
            "model": cfg.model.as_deref(),
            "backend": backend_name(cfg.backend),
            "agent": cfg.agent.as_str(),
            "chunker": chunker_name(cfg.chunker),
            "schema_mode": schema_mode_name(cfg.schema_mode),
            "schema": cfg.schema.as_deref(),
            "preset": cfg.preset.as_deref(),
            "preset_file": cfg.preset_file.as_deref(),
            "template_loaded": template_loaded,
            "lang": cfg.lang.as_deref(),
            "max_rounds": cfg.max_rounds,
            "merge_strategy": merge_strategy_name(cfg.merge_strategy),
            "coref": cfg.coref,
            "max_concurrency": cfg.max_concurrency,
            "relation_gleaning": cfg.relation_gleaning,
            "output": output_name(cfg.output)
        }
    })
}

fn print_dry_run(args: &Args, cfg: &Resolved, template_loaded: bool) -> anyhow::Result<()> {
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&dry_run_value(args, cfg, template_loaded))?
        );
    } else {
        println!("kg-extract dry-run");
        println!("  input: {}", args.file.as_deref().unwrap_or("stdin"));
        println!("  input_format: {}", input_format_name(args.input_format));
        println!("  engine: {}", engine_name(cfg.engine));
        println!("  backend: {}", backend_name(cfg.backend));
        println!("  agent: {}", cfg.agent);
        println!("  output: {}", output_name(cfg.output));
        println!("  will_read_input: false");
        println!("  will_call_backend: false");
    }
    Ok(())
}

/// Render the extracted graph in the requested output format to stdout. The
/// seven formats are mutually-exclusive terminal printing, split out of `main`
/// so the dispatch arm-per-format complexity lives in one tested-by-wiring
/// place rather than inflating main's cyclomatic complexity.
fn print_response(fmt: OutFmt, response: &ExtractionResponse) -> anyhow::Result<()> {
    match fmt {
        OutFmt::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&response.knowledge_graph.to_dict())?
            )
        }
        OutFmt::Jsonl => {
            for (_, entity) in response.knowledge_graph.entities.iter() {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "kind": "entity",
                        "data": entity.to_dict(),
                    }))?
                );
            }
            for triple in &response.knowledge_graph.triples {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "kind": "triple",
                        "data": triple.to_dict(),
                    }))?
                );
            }
        }
        OutFmt::KgProtocol => {
            println!(
                "{}",
                serde_json::to_string_pretty(&response.knowledge_graph.to_kg_document())?
            )
        }
        OutFmt::NodeLink => {
            println!(
                "{}",
                serde_json::to_string_pretty(&response.knowledge_graph.to_node_link())?
            )
        }
        OutFmt::LadybugImport => {
            println!(
                "{}",
                serde_json::to_string_pretty(&kg_extract::ladybug_export::to_ladybug_import_json(
                    &response.knowledge_graph
                ))?
            )
        }
        OutFmt::Mermaid => println!("{}", response.get_mermaid_code()),
        OutFmt::Stats => println!("{}", serde_json::to_string_pretty(&response.get_stats())?),
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let matches = Args::command().get_matches();
    let args = Args::from_arg_matches(&matches)?;

    if args.describe {
        print_describe(args.json)?;
        return Ok(());
    }

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

    if args.dry_run {
        print_dry_run(&args, &cfg, template.is_some())?;
        return Ok(());
    }

    if args.json {
        anyhow::bail!("--json is only supported with --describe or --dry-run; use -o json for extraction output");
    }

    let text = read_input(&args.file)?;
    if text.trim().is_empty() {
        anyhow::bail!("no input text (provide --file or pipe via stdin)");
    }
    // Pre-chunked input (chonkie chunks): parse up front so a malformed file
    // fails fast, before any backend is constructed.
    let prechunked = match args.input_format {
        InputFormat::Text => None,
        InputFormat::Chunks => Some(
            kg_extract::chunking::parse_prechunked(&text)
                .context("parsing --input-format chunks input")?,
        ),
    };
    // Document identity for provenance citations: with pre-chunked input the
    // chunks' recorded source document (`-f` names the chunks file, not the
    // document); otherwise the input file path, or none when reading stdin.
    let source_doc: Option<String> = match &prechunked {
        Some(p) => p.source.clone(),
        None => args.file.as_ref().filter(|f| f.as_str() != "-").cloned(),
    };
    // The agentic engine drives the SDK client itself (cwd sandbox + one
    // long-lived session), so it bypasses `make_backend` and its `--backend`
    // value — the provider is chosen by `--agent`.
    let extractor: Box<dyn Extractor + Send + Sync> = if matches!(cfg.engine, Engine::Agentic) {
        let mut c = AgenticExtractor::default_config();
        c.chunker = cfg.chunker.into();
        c.source_doc = source_doc.clone();
        if let Some(m) = &cfg.model {
            c.model_name = m.clone();
        }
        // Fixed mode validates each slice against this schema and drops
        // out-of-schema records; Open/Evolving leave it as hints.
        if let Some(path) = &cfg.schema {
            c.spec.schema = Schema::from_json_file(expand_tilde(path))
                .with_context(|| format!("loading --schema {path}"))?;
        }
        Box::new(
            AgenticExtractor::with_config(&cfg.agent, c)
                .schema_mode(cfg.schema_mode.into())
                .relation_gleanings(cfg.relation_gleaning),
        )
    } else {
        let backend = make_backend(
            cfg.backend,
            &cfg.agent,
            args.mock_response.as_deref(),
            args.mock_tool_calls.as_deref(),
        )?;
        let coref_mode = if cfg.coref {
            CorefMode::Fuzzy
        } else {
            CorefMode::Off
        };
        match cfg.engine {
            Engine::Simple => {
                let mut c = SimpleExtractor::default_config();
                c.chunker = cfg.chunker.into();
                c.source_doc = source_doc.clone();
                c.max_concurrency = cfg.max_concurrency;
                c.spec.merge_strategy = cfg.merge_strategy.into();
                c.spec.coref = coref_mode;
                if let Some(m) = &cfg.model {
                    c.model_name = m.clone();
                }
                Box::new(
                    SimpleExtractor::with_config(backend, c)
                        .relation_gleanings(cfg.relation_gleaning),
                )
            }
            Engine::SchemaJson => {
                let mut c = SchemaJsonExtractor::default_config();
                c.chunker = cfg.chunker.into();
                c.source_doc = source_doc.clone();
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
                Box::new(
                    SchemaJsonExtractor::with_config(backend, c)
                        .schema_mode(cfg.schema_mode.into()),
                )
            }
            Engine::Toolcall => {
                let mut c = ToolCallExtractor::default_config();
                c.chunker = cfg.chunker.into();
                c.source_doc = source_doc.clone();
                c.spec.merge_strategy = cfg.merge_strategy.into();
                c.spec.coref = coref_mode;
                if let Some(m) = &cfg.model {
                    c.model_name = m.clone();
                }
                if let Some(path) = &cfg.schema {
                    c.spec.schema = Schema::from_json_file(expand_tilde(path))
                        .with_context(|| format!("loading --schema {path}"))?;
                }
                Box::new(
                    ToolCallExtractor::with_config(backend, c)
                        .schema_mode(cfg.schema_mode.into())
                        .max_rounds(cfg.max_rounds),
                )
            }
            Engine::Agentic => unreachable!("agentic handled above"),
        }
    };
    let mut response = match &prechunked {
        Some(p) => extractor.extract_prechunked(&p.segments).await?,
        None => extractor.extract(&text).await?,
    };

    // Audit out-of-vocabulary type tokens (aliased / fell back to OTHER) into
    // the response metadata, and surface a one-line summary so the silent
    // normalisation is visible without parsing the JSON.
    if response.annotate_type_normalization() {
        if let Some(c) = response
            .metadata
            .get("type_normalization")
            .and_then(|v| v.get("counts"))
        {
            eprintln!("Type normalization: {c}");
        }
    }

    print_response(cfg.output, &response)?;
    Ok(())
}

#[cfg(test)]
#[path = "kg-extract_test.rs"]
mod tests;
