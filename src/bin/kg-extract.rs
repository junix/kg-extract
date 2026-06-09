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

use kg_extract::backend::{AgentCli, AgentCliBackend, LlmBackend, MockBackend};
use kg_extract::extractor::{
    Extractor, SimpleExtractor, ToolCallExtractor, TriplexExtractor, YoutuExtractor, YoutuMode,
};
use kg_extract::types::ChunkStrategy;

#[derive(Copy, Clone, Debug, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Engine {
    Simple,
    Triplex,
    Youtu,
    Toolcall,
}

#[derive(Copy, Clone, Debug, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Backend {
    /// In-process `llms` crate (requires the `llms-backend` feature).
    Llms,
    /// Subprocess agent CLI: minimaxcc / glmcc / mimocc.
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

#[derive(Copy, Clone, Debug, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
enum OutFmt {
    Json,
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
    youtu_agent: Option<bool>,
    community: Option<bool>,
    toolcall_agent: Option<bool>,
    max_rounds: Option<usize>,
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

    /// Agent CLI to use when --backend agent (default minimaxcc).
    #[arg(long, default_value = "minimaxcc")]
    agent: String,

    /// Chunking strategy.
    #[arg(short = 'k', long, value_enum, default_value_t = Chunker::Recursive)]
    chunker: Chunker,

    /// Youtu agent mode (schema evolution).
    #[arg(long)]
    youtu_agent: bool,

    /// Enable community detection (Youtu engine).
    #[arg(long)]
    community: bool,

    /// Tool-call engine: allow schema evolution (drops enum constraints).
    #[arg(long)]
    toolcall_agent: bool,

    /// Tool-call engine: max tool-calling rounds (1 = single-round collection).
    #[arg(long, default_value_t = 1)]
    max_rounds: usize,

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
    youtu_agent: bool,
    community: bool,
    toolcall_agent: bool,
    max_rounds: usize,
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
        youtu_agent: pick(m, "youtu_agent", args.youtu_agent, cfg.youtu_agent),
        community: pick(m, "community", args.community, cfg.community),
        toolcall_agent: pick(m, "toolcall_agent", args.toolcall_agent, cfg.toolcall_agent),
        max_rounds: pick(m, "max_rounds", args.max_rounds, cfg.max_rounds),
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
        Some(path) => Ok(std::fs::read_to_string(path)?),
    }
}

fn make_backend(
    backend: Backend,
    agent: &str,
    mock_response: Option<&str>,
) -> anyhow::Result<Arc<dyn LlmBackend>> {
    match backend {
        Backend::Agent => {
            let cli = AgentCli::parse(agent)
                .ok_or_else(|| anyhow::anyhow!("unknown agent CLI: {}", agent))?;
            Ok(Arc::new(AgentCliBackend::new(cli)))
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let matches = Args::command().get_matches();
    let args = Args::from_arg_matches(&matches)?;

    let file_cfg = load_config(args.config.as_deref())?;
    let cfg = resolve(&matches, &args, file_cfg);

    let text = read_input(&args.file)?;
    if text.trim().is_empty() {
        anyhow::bail!("no input text (provide --file or pipe via stdin)");
    }
    let backend = make_backend(cfg.backend, &cfg.agent, args.mock_response.as_deref())?;

    let response = match cfg.engine {
        Engine::Simple => {
            let mut c = SimpleExtractor::default_config();
            c.chunker = cfg.chunker.into();
            if let Some(m) = &cfg.model {
                c.model_name = m.clone();
            }
            SimpleExtractor::with_config(backend, c).extract(&text).await?
        }
        Engine::Triplex => {
            let mut c = TriplexExtractor::default_config();
            c.chunker = cfg.chunker.into();
            if let Some(m) = &cfg.model {
                c.model_name = m.clone();
            }
            TriplexExtractor::with_config(backend, c).extract(&text).await?
        }
        Engine::Youtu => {
            let mut c = YoutuExtractor::default_config();
            c.chunker = cfg.chunker.into();
            if let Some(m) = &cfg.model {
                c.model_name = m.clone();
            }
            let mode = if cfg.youtu_agent { YoutuMode::Agent } else { YoutuMode::NoAgent };
            YoutuExtractor::with_config(backend, c)
                .mode(mode)
                .community_detection(cfg.community)
                .extract(&text)
                .await?
        }
        Engine::Toolcall => {
            let mut c = ToolCallExtractor::default_config();
            c.chunker = cfg.chunker.into();
            if let Some(m) = &cfg.model {
                c.model_name = m.clone();
            }
            ToolCallExtractor::with_config(backend, c)
                .schema_evolution(cfg.toolcall_agent)
                .max_rounds(cfg.max_rounds)
                .extract(&text)
                .await?
        }
    };

    match cfg.output {
        OutFmt::Json => {
            println!("{}", serde_json::to_string_pretty(&response.knowledge_graph.to_dict())?)
        }
        OutFmt::Mermaid => println!("{}", response.get_mermaid_code()),
        OutFmt::Stats => println!("{}", serde_json::to_string_pretty(&response.get_stats())?),
    }
    Ok(())
}

#[cfg(test)]
#[path = "kg-extract_test.rs"]
mod tests;
