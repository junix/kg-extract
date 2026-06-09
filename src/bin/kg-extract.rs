//! CLI for kg-extract: extract a knowledge graph from text and emit JSON or
//! Mermaid. Mirrors `python -m graph.kg_extractor`.

use std::io::Read;
use std::sync::Arc;

use clap::{Parser, ValueEnum};
use kg_extract::backend::{AgentCli, AgentCliBackend, LlmBackend, MockBackend};
use kg_extract::extractor::{
    Extractor, SimpleExtractor, ToolCallExtractor, TriplexExtractor, YoutuExtractor, YoutuMode,
};
use kg_extract::types::ChunkStrategy;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Engine {
    Simple,
    Triplex,
    Youtu,
    Toolcall,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Backend {
    /// In-process `llms` crate (requires the `llms-backend` feature).
    Llms,
    /// Subprocess agent CLI: minimaxcc / glmcc / mimocc.
    Agent,
    /// Deterministic mock (reads a canned response from --mock-response).
    Mock,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
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

#[derive(Copy, Clone, Debug, ValueEnum)]
enum OutFmt {
    Json,
    Mermaid,
    Stats,
}

/// Extract a knowledge graph from text.
#[derive(Parser, Debug)]
#[command(name = "kg-extract", version, about)]
struct Args {
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
    #[arg(short, long, value_enum, default_value_t = Chunker::Recursive)]
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

fn make_backend(args: &Args) -> anyhow::Result<Arc<dyn LlmBackend>> {
    match args.backend {
        Backend::Agent => {
            let cli = AgentCli::parse(&args.agent)
                .ok_or_else(|| anyhow::anyhow!("unknown agent CLI: {}", args.agent))?;
            Ok(Arc::new(AgentCliBackend::new(cli)))
        }
        Backend::Mock => {
            let resp = args.mock_response.clone().unwrap_or_default();
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
    let args = Args::parse();
    let text = read_input(&args.file)?;
    if text.trim().is_empty() {
        anyhow::bail!("no input text (provide --file or pipe via stdin)");
    }
    let backend = make_backend(&args)?;

    let response = match args.engine {
        Engine::Simple => {
            let mut cfg = SimpleExtractor::default_config();
            cfg.chunker = args.chunker.into();
            if let Some(m) = &args.model {
                cfg.model_name = m.clone();
            }
            SimpleExtractor::with_config(backend, cfg).extract(&text).await?
        }
        Engine::Triplex => {
            let mut cfg = TriplexExtractor::default_config();
            cfg.chunker = args.chunker.into();
            if let Some(m) = &args.model {
                cfg.model_name = m.clone();
            }
            TriplexExtractor::with_config(backend, cfg).extract(&text).await?
        }
        Engine::Youtu => {
            let mut cfg = YoutuExtractor::default_config();
            cfg.chunker = args.chunker.into();
            if let Some(m) = &args.model {
                cfg.model_name = m.clone();
            }
            let mode = if args.youtu_agent { YoutuMode::Agent } else { YoutuMode::NoAgent };
            YoutuExtractor::with_config(backend, cfg)
                .mode(mode)
                .community_detection(args.community)
                .extract(&text)
                .await?
        }
        Engine::Toolcall => {
            let mut cfg = ToolCallExtractor::default_config();
            cfg.chunker = args.chunker.into();
            if let Some(m) = &args.model {
                cfg.model_name = m.clone();
            }
            ToolCallExtractor::with_config(backend, cfg)
                .schema_evolution(args.toolcall_agent)
                .max_rounds(args.max_rounds)
                .extract(&text)
                .await?
        }
    };

    match args.output {
        OutFmt::Json => println!("{}", serde_json::to_string_pretty(&response.knowledge_graph.to_dict())?),
        OutFmt::Mermaid => println!("{}", response.get_mermaid_code()),
        OutFmt::Stats => println!("{}", serde_json::to_string_pretty(&response.get_stats())?),
    }
    Ok(())
}
