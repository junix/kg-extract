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
//!     cargo run --features mcp --bin kg-extract-mcp -- -o /path/to/out --source-root /path/to/docs
//!
//! All protocol traffic is JSON-RPC 2.0 over stdin/stdout — never write to
//! stdout yourself; diagnostics go to stderr.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use kg_extract::mcp::{KgStore, SchemaPolicy, SourceCitation};
use kg_extract::types::{Schema, SchemaMode};

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
    /// Source document path, relative to the server source_root, for provenance citation.
    #[serde(default)]
    source_file: Option<String>,
    /// 1-based inclusive start line for provenance citation.
    #[serde(default)]
    start_line: Option<usize>,
    /// 1-based inclusive end line for provenance citation.
    #[serde(default)]
    end_line: Option<usize>,
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
    /// Source document path, relative to the server source_root, for provenance citation.
    #[serde(default)]
    source_file: Option<String>,
    /// 1-based inclusive start line for provenance citation.
    #[serde(default)]
    start_line: Option<usize>,
    /// 1-based inclusive end line for provenance citation.
    #[serde(default)]
    end_line: Option<usize>,
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

#[derive(Debug, Deserialize, JsonSchema)]
struct ProposeSchemaTypeParams {
    /// Output key: the graph is stored at <output>/<path>.json
    path: String,
    /// Type category: "node", "relation", or "attribute"
    kind: String,
    /// Proposed type name
    name: String,
    /// Optional explanation for why this schema type is needed
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct QuerySchemaParams {
    /// Output key whose graph-specific proposals should be included
    path: String,
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
        Self {
            store: std::sync::Arc::new(store),
            tool_router: Self::tool_router(),
        }
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

fn citation_from_parts(
    source_file: Option<String>,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<Option<SourceCitation>, String> {
    match (source_file, start_line, end_line) {
        (None, None, None) => Ok(None),
        (Some(source_file), Some(start_line), Some(end_line)) => {
            SourceCitation::new(source_file, start_line, end_line)
                .map(Some)
                .map_err(|e| e.to_string())
        }
        _ => Err(
            "source_file, start_line, and end_line must be provided together for provenance".into(),
        ),
    }
}

#[tool_router]
impl KgExtractMcp {
    #[tool(
        description = "Record an entity in the knowledge graph at <output>/<path>.json. Merges by name into any existing entity. Provide source_file, start_line, and end_line together to store provenance in metadata.citations. source_file must be a relative path under the server source_root; absolute paths and '..' are rejected. The server validates that the file exists and that the line range is in bounds before writing. Returns the updated graph stats and the file path written."
    )]
    async fn add_entity(
        &self,
        Parameters(p): Parameters<AddEntityParams>,
    ) -> Result<String, String> {
        let attrs = p.attributes.unwrap_or_default();
        let kind = p.kind.as_deref().unwrap_or("OTHER");
        let citation = citation_from_parts(p.source_file, p.start_line, p.end_line)?;
        let r = self
            .store
            .add_entity_with_citation(&p.path, &p.name, kind, p.description, attrs, citation)
            .map_err(|e| e.to_string())?;
        json_string(r)
    }

    #[tool(
        description = "Record a relationship between two entities in the graph at <output>/<path>.json. BOTH endpoints must already exist — call add_entity for each first. Provide source_file, start_line, and end_line together to store relationship provenance in metadata.citations. source_file must be a relative path under the server source_root; absolute paths and '..' are rejected. The server validates that the file exists and that the line range is in bounds before writing. If an endpoint is missing the tool returns an error naming it and listing the known entities, so you can add it (or fix a typo) and retry. Identical relations are deduplicated and citations are merged. Returns updated stats and the file path."
    )]
    async fn add_relation(
        &self,
        Parameters(p): Parameters<AddRelationParams>,
    ) -> Result<String, String> {
        let r = self
            .store
            .add_relation_with_citation(
                &p.path,
                &p.source,
                &p.predicate,
                &p.target,
                p.description,
                p.strength,
                citation_from_parts(p.source_file, p.start_line, p.end_line)?,
            )
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

    #[tool(
        description = "In evolving schema mode, propose a new schema type for this graph path before using it. `kind` must be 'node', 'relation', or 'attribute'. The proposal is persisted under graph metadata and later add_entity/add_relation/add_attribute calls may use it. Returns an error outside evolving mode."
    )]
    async fn propose_schema_type(
        &self,
        Parameters(p): Parameters<ProposeSchemaTypeParams>,
    ) -> Result<String, String> {
        let r = self
            .store
            .propose_schema_type(&p.path, &p.kind, &p.name, p.reason)
            .map_err(|e| e.to_string())?;
        json_string(r)
    }

    #[tool(
        description = "Return the server schema policy, seed schema, and graph-specific proposed schema types for a path. Use this before writing when the server runs in fixed or evolving schema mode."
    )]
    async fn query_schema(
        &self,
        Parameters(p): Parameters<QuerySchemaParams>,
    ) -> Result<String, String> {
        let r = self
            .store
            .query_schema(&p.path)
            .map_err(|e| e.to_string())?;
        json_string(r)
    }
}

#[tool_handler]
impl ServerHandler for KgExtractMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "kg-extract-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Incremental knowledge-graph builder. Every tool takes a `path` arg; \
                 results are merged into <output>/<path>.json. Workflow: for each chunk of \
                 text, call add_entity for every entity, then add_relation between them \
                 (both endpoints must already exist), and add_attribute for extra facts. \
                 When a fact comes from source text, pass source_file, start_line, and \
                 end_line together on add_entity/add_relation so provenance is stored in \
                 metadata.citations. source_file must be relative to source_root; the \
                 server validates file existence and line bounds before writing. \
                 Use query_graph (view=entities/relations/neighbors) to see what is already \
                 recorded before extending. Use query_schema to inspect fixed/evolving schema \
                 constraints; in evolving mode call propose_schema_type before using a new \
                 entity, relation, or attribute type. The server stores and deduplicates; it \
                 does not call an LLM itself.",
            )
    }
}

// ── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, ValueEnum)]
#[value(rename_all = "lowercase")]
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

    /// Source document root. source_file provenance values must be relative to
    /// this directory. Defaults to the current working directory.
    #[arg(long)]
    source_root: Option<String>,

    /// Schema mode: open accepts any type; fixed accepts only --schema; evolving
    /// starts from --schema and allows explicit propose_schema_type additions.
    #[arg(long, value_enum, default_value_t = SchemaModeArg::Open)]
    schema_mode: SchemaModeArg,

    /// Schema JSON file. Required for --schema-mode fixed|evolving.
    #[arg(long)]
    schema: Option<String>,

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

fn load_schema_policy(
    mode_arg: SchemaModeArg,
    schema_arg: Option<&str>,
) -> anyhow::Result<SchemaPolicy> {
    let mode: SchemaMode = mode_arg.into();
    let schema = match schema_arg {
        Some(path) => Schema::from_json_file(expand_tilde(path))
            .with_context(|| format!("loading --schema {path}"))?,
        None => Schema::default(),
    };
    SchemaPolicy::new(mode, schema)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    validate_config(args.config.as_deref())?;
    let policy = load_schema_policy(args.schema_mode, args.schema.as_deref())?;

    let output = expand_tilde(&args.output);
    std::fs::create_dir_all(&output)
        .with_context(|| format!("creating output dir {}", output.display()))?;
    let source_root = match args.source_root.as_deref() {
        Some(path) => expand_tilde(path),
        None => std::env::current_dir().context("resolving current directory as source root")?,
    };
    let source_root = source_root
        .canonicalize()
        .with_context(|| format!("resolving source root {}", source_root.display()))?;
    if !source_root.is_dir() {
        anyhow::bail!("source root is not a directory: {}", source_root.display());
    }
    if args.verbose {
        eprintln!(
            "kg-extract-mcp: serving stdio MCP; output dir = {}; source root = {}; schema mode = {:?}",
            output.display(),
            source_root.display(),
            args.schema_mode
        );
    }

    KgExtractMcp::new(KgStore::with_policy_and_source_root(
        output,
        policy,
        source_root,
    ))
    .serve_stdio()
    .await
}
