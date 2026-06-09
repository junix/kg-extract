//! `kg-extract-mcp` — a stdio MCP server exposing function-call style knowledge-
//! graph building tools.
//!
//! Unlike `kg-extract` (which runs an extraction engine against text), this
//! server calls **no** LLM. An MCP *client* (an LLM/agent) reads the text itself
//! and drives `add_entity` / `add_relation` / `add_attribute`; each call merges a
//! single mutation into `<output>/<path>.json`. The graph accumulates across
//! calls, deduplicated by entity id and triple.
//!
//! Built with the `rmcp` SDK behind the `mcp` feature:
//!     cargo run --features mcp --bin kg-extract-mcp -- -o /path/to/out
//!
//! All protocol traffic is JSON-RPC 2.0 over stdin/stdout — never write to
//! stdout yourself; diagnostics go to stderr.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use kg_extract::mcp::KgStore;

// ── Tool parameter types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
struct AddEntityParams {
    /// Output key: the graph is stored at <output>/<path>.json
    path: String,
    /// Entity name, as it appears in the text (also its identity for merging)
    name: String,
    /// Entity type, e.g. person, organization, location, product, technology, date
    #[serde(default, rename = "type")]
    kind: Option<String>,
    /// 1-2 sentence description
    #[serde(default)]
    description: Option<String>,
    /// Optional key/value attributes to attach to the entity
    #[serde(default)]
    attributes: Option<HashMap<String, Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AddRelationParams {
    /// Output key: the graph is stored at <output>/<path>.json
    path: String,
    /// Source entity name (must already exist — call add_entity first)
    source: String,
    /// Relationship type, e.g. works_at, located_in, uses, part_of
    predicate: String,
    /// Target entity name (must already exist — call add_entity first)
    target: String,
    /// Optional free-text description of the relationship
    #[serde(default)]
    description: Option<String>,
    /// Optional confidence in 0..1 (defaults to 0.8)
    #[serde(default)]
    strength: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AddAttributeParams {
    /// Output key: the graph is stored at <output>/<path>.json
    path: String,
    /// Name of a previously added entity
    entity: String,
    /// Attribute key
    key: String,
    /// Attribute value (string, number, boolean, object, ...)
    value: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct QueryGraphParams {
    /// Output key: the graph at <output>/<path>.json to query
    path: String,
    /// What to return: "summary" (counts only, default), "entities", "relations", "neighbors", or "full"
    #[serde(default)]
    view: Option<String>,
    /// Focal entity name — required when view="neighbors"; returns that entity plus its incoming/outgoing relations
    #[serde(default)]
    entity: Option<String>,
    /// Max items for list views (default 200)
    #[serde(default)]
    limit: Option<usize>,
}

// ── MCP server ───────────────────────────────────────────────────────────────

#[derive(Clone)]
struct KgExtractMcp {
    store: std::sync::Arc<KgStore>,
    // Read by the `#[tool_handler]` macro-generated dispatch; dead-code analysis
    // can't see through the macro, so silence the false positive.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl KgExtractMcp {
    fn new(store: KgStore) -> Self {
        Self { store: std::sync::Arc::new(store), tool_router: Self::tool_router() }
    }

    async fn serve_stdio(self) -> anyhow::Result<()> {
        let transport = rmcp::transport::stdio();
        self.serve(transport).await?.waiting().await?;
        Ok(())
    }
}

fn json_string(v: Value) -> Result<String, String> {
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

#[tool_router]
impl KgExtractMcp {
    #[tool(
        description = "Record an entity in the knowledge graph at <output>/<path>.json. Merges by name into any existing entity. Returns the updated graph stats and the file path written."
    )]
    async fn add_entity(
        &self,
        Parameters(p): Parameters<AddEntityParams>,
    ) -> Result<String, String> {
        let attrs = p.attributes.unwrap_or_default();
        let kind = p.kind.as_deref().unwrap_or("OTHER");
        let r = self
            .store
            .add_entity(&p.path, &p.name, kind, p.description, attrs)
            .map_err(|e| e.to_string())?;
        json_string(r)
    }

    #[tool(
        description = "Record a relationship between two entities in the graph at <output>/<path>.json. BOTH endpoints must already exist — call add_entity for each first. If an endpoint is missing the tool returns an error naming it and listing the known entities, so you can add it (or fix a typo) and retry. Identical relations are deduplicated. Returns updated stats and the file path."
    )]
    async fn add_relation(
        &self,
        Parameters(p): Parameters<AddRelationParams>,
    ) -> Result<String, String> {
        let r = self
            .store
            .add_relation(&p.path, &p.source, &p.predicate, &p.target, p.description, p.strength)
            .map_err(|e| e.to_string())?;
        json_string(r)
    }

    #[tool(
        description = "Attach a key/value attribute to a previously added entity in the graph at <output>/<path>.json. If the entity does not exist the tool returns an error listing the known entities — add it first (or fix the name) and retry."
    )]
    async fn add_attribute(
        &self,
        Parameters(p): Parameters<AddAttributeParams>,
    ) -> Result<String, String> {
        let r = self
            .store
            .add_attribute(&p.path, &p.entity, &p.key, p.value)
            .map_err(|e| e.to_string())?;
        json_string(r)
    }

    #[tool(
        description = "Query the knowledge graph at <output>/<path>.json. `view` controls what comes back: 'summary' (counts only, default — cheapest), 'entities' (id/label/type list), 'relations' (source/predicate/target list), 'neighbors' (a focal `entity` plus its incoming & outgoing relations — use to check what you already know before extending), or 'full'. `limit` caps list sizes (default 200). Returns an error if no graph exists at that path, the view is unknown, or a neighbors query names a missing entity."
    )]
    async fn query_graph(
        &self,
        Parameters(p): Parameters<QueryGraphParams>,
    ) -> Result<String, String> {
        let view = p.view.as_deref().unwrap_or("summary");
        let limit = p.limit.unwrap_or(200);
        let r = self
            .store
            .query_graph(&p.path, view, p.entity.as_deref(), limit)
            .map_err(|e| e.to_string())?;
        json_string(r)
    }
}

#[tool_handler]
impl ServerHandler for KgExtractMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("kg-extract-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Incremental knowledge-graph builder. Every tool takes a `path` arg; \
                 results are merged into <output>/<path>.json. Workflow: for each chunk of \
                 text, call add_entity for every entity, then add_relation between them \
                 (both endpoints must already exist), and add_attribute for extra facts. \
                 Use query_graph (view=entities/relations/neighbors) to see what is already \
                 recorded before extending. The server stores and deduplicates; it does not \
                 call an LLM itself.",
            )
    }
}

// ── CLI ──────────────────────────────────────────────────────────────────────

/// stdio MCP server for incremental knowledge-graph building.
#[derive(Parser, Debug)]
#[command(name = "kg-extract-mcp", version, about)]
struct Args {
    /// Config file path, or inline JSON. Defaults to ~/.kg-extract/config.json when
    /// present. Accepted for parity with `kg-extract`; the store tools are
    /// deterministic and do not consult engine/backend/model fields.
    #[arg(short = 'c', long)]
    config: Option<String>,

    /// Output directory. Each tool call writes <output>/<path>.json.
    #[arg(short = 'o', long)]
    output: String,

    /// Verbose diagnostics to stderr.
    #[arg(short = 'v', long)]
    verbose: bool,
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

/// Validate `--config` (inline JSON, a path, or the default file). Errors on bad
/// explicit input; a missing default file is fine. The parsed value is not used
/// by the store tools — this is purely CLI parity with `kg-extract`.
fn validate_config(arg: Option<&str>) -> anyhow::Result<()> {
    let body = match arg {
        Some(s) if s.trim_start().starts_with('{') => s.to_string(),
        Some(path) => {
            let path = expand_tilde(path);
            std::fs::read_to_string(&path)
                .with_context(|| format!("reading config file {}", path.display()))?
        }
        None => match default_config_path() {
            Some(p) if p.exists() => std::fs::read_to_string(&p)
                .with_context(|| format!("reading config file {}", p.display()))?,
            _ => return Ok(()),
        },
    };
    serde_json::from_str::<Value>(&body).context("parsing --config JSON")?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    validate_config(args.config.as_deref())?;

    let output = expand_tilde(&args.output);
    std::fs::create_dir_all(&output)
        .with_context(|| format!("creating output dir {}", output.display()))?;
    if args.verbose {
        eprintln!("kg-extract-mcp: serving stdio MCP; output dir = {}", output.display());
    }

    KgExtractMcp::new(KgStore::new(output)).serve_stdio().await
}
