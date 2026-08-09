//! kg.provider/v1 surface: `describe` / `available` / `invoke`.
//!
//! Lets an external capability hub (kg-acme) discover and call kg-extract
//! purely data-driven:
//!
//! - [`describe_document`] — the `kg.provider/v1` manifest: provider identity
//!   plus one entry per capability (`capability_id`, `input_schema`,
//!   `side_effects`, `output`, and a `cli_spec` argv template whose flag
//!   semantics follow the acme CLIFlag convention — emission order is
//!   `always ++ subcommand ++ positionals ++ flags(by order, tiebreak flag)`).
//! - [`available_report`] — read-only availability probe (env vars / PATH /
//!   compiled features; never a network call). The CLI exits 0 and reports
//!   misses inside the JSON.
//! - [`invoke`] — run one capability from a JSON request and return a
//!   `kg.execution/v1` envelope (`status`/`result`/`artifacts`/`diagnostics`/
//!   `error`). Human-readable progress goes to stderr; stdout carries exactly
//!   one envelope.
//!
//! Capability inventory (ids are stable — hubs match on them):
//!
//! | capability_id | in → out | side effects |
//! |---|---|---|
//! | `extract.entities_relations` | text/file → `kg-document` artifact | network, data_egress |
//! | `detect.communities` | kg-document → communities JSON | — |
//! | `detect.communities_hierarchy` | kg-document → hierarchy JSON | — |
//! | `summarize.communities` | kg-document → communities+summaries | network, data_egress |
//! | `resolve.coref` | kg-document → kg-document artifact | — |
//! | `resolve.canonical_direction` | kg-document → kg-document artifact | — |
//!
//! `resolve.coref` / `resolve.canonical_direction` also exist as options on
//! `extract.entities_relations` (the `coref` / `canonical_direction` request
//! fields, mirroring the CLI flags); the standalone forms are graph-in/
//! graph-out so a hub can chain them after any graph producer.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::backend::{
    LlmBackend, MockBackend, PiAgentBackend, SdkAgentBackend, ToolInvocation,
};
use crate::extractor::{
    AgenticExtractor, Extractor, SchemaJsonExtractor, SchemaMode, SimpleExtractor,
    ToolCallExtractor,
};
use crate::template::{gallery, TemplateCfg};
use crate::types::{ChunkStrategy, CorefMode, KnowledgeGraph, MergeStrategy, Schema};

/// Provider-protocol identity.
pub const PROVIDER_PROTOCOL: &str = "kg.provider/v1";
/// Envelope protocol emitted by [`invoke`].
pub const EXECUTION_PROTOCOL: &str = "kg.execution/v1";
/// Stable provider id hubs match on.
pub const PROVIDER_ID: &str = "kg-extract";

/// Stable capability ids (hub match keys — do not rename).
pub const CAP_EXTRACT: &str = "extract.entities_relations";
pub const CAP_DETECT_COMMUNITIES: &str = "detect.communities";
pub const CAP_DETECT_HIERARCHY: &str = "detect.communities_hierarchy";
pub const CAP_SUMMARIZE: &str = "summarize.communities";
pub const CAP_RESOLVE_COREF: &str = "resolve.coref";
pub const CAP_RESOLVE_DIRECTION: &str = "resolve.canonical_direction";

/// All capability ids, in describe order.
pub const CAPABILITY_IDS: [&str; 6] = [
    CAP_EXTRACT,
    CAP_DETECT_COMMUNITIES,
    CAP_DETECT_HIERARCHY,
    CAP_SUMMARIZE,
    CAP_RESOLVE_COREF,
    CAP_RESOLVE_DIRECTION,
];

// ---------------------------------------------------------------------------
// Shared backend construction (moved out of the CLI so invoke reuses it).
// ---------------------------------------------------------------------------

/// Build a completion backend by name (`llms` / `agent` / `mock`) — the same
/// resolution the CLI's `--backend` performs. `llms` requires the
/// `llms-backend` feature; `agent` dispatches pi-rs's `pi-agent` to
/// [`PiAgentBackend`] and everything else to the stream-json
/// [`SdkAgentBackend`]; `mock` replays canned responses.
pub fn make_backend(
    backend: &str,
    agent: &str,
    mock_response: Option<&str>,
    mock_tool_calls: Option<&str>,
) -> anyhow::Result<Arc<dyn LlmBackend>> {
    match backend {
        "agent" => {
            // pi-agent (from pi-rs) has a different CLI contract than the
            // Claude-Code wrappers, so it gets its own backend. Everything else
            // is driven through the structured stream-json SDK.
            if PiAgentBackend::accepts(agent) {
                return Ok(Arc::new(PiAgentBackend::new()));
            }
            Ok(Arc::new(SdkAgentBackend::for_agent(agent)?))
        }
        "mock" => {
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
        "llms" => {
            #[cfg(feature = "llms-backend")]
            {
                Ok(Arc::new(crate::backend::LlmsBackend::new()))
            }
            #[cfg(not(feature = "llms-backend"))]
            {
                anyhow::bail!(
                    "the `llms` backend requires building with --features llms-backend; \
                     use backend `agent` or `mock` instead"
                )
            }
        }
        other => anyhow::bail!("unknown backend '{other}' (expected llms / agent / mock)"),
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

/// Parse scripted tool-call rounds for the mock backend (inline JSON or a file
/// path). Shape: `[{"name": "...", "arguments": {...}}]` for one round, or
/// `[[...], [...]]` for many rounds.
pub fn parse_mock_tool_rounds(input: &str) -> anyhow::Result<Vec<Vec<ToolInvocation>>> {
    let raw = if input.trim_start().starts_with('[') {
        input.to_string()
    } else {
        let path = expand_tilde(input);
        std::fs::read_to_string(&path)
            .with_context(|| format!("reading mock_tool_calls {}", path.display()))?
    };
    let value: serde_json::Value =
        serde_json::from_str(&raw).context("parsing mock_tool_calls JSON")?;
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

fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

// ---------------------------------------------------------------------------
// describe
// ---------------------------------------------------------------------------

fn str_prop(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}

fn bool_prop(desc: &str, default: bool) -> Value {
    json!({"type": "boolean", "description": desc, "default": default})
}

fn int_prop(desc: &str, default: u64, minimum: u64) -> Value {
    json!({"type": "integer", "description": desc, "default": default, "minimum": minimum})
}

/// Enum property as `oneOf` consts so every value carries its own description
/// (self-explanatory even with the field name stripped).
fn enum_prop(desc: &str, variants: &[(&str, &str)], default: &str) -> Value {
    let one_of: Vec<Value> = variants
        .iter()
        .map(|(v, d)| json!({"const": v, "description": d}))
        .collect();
    json!({"description": desc, "default": default, "oneOf": one_of})
}

/// One cli_spec flag entry (acme CLIFlag semantics).
fn cli_flag(name: &str, flag: &str, kind: &str, optional: bool, order: u32) -> Value {
    json!({"name": name, "flag": flag, "kind": kind, "optional": optional, "order": order})
}

fn cli_flag_default(
    name: &str,
    flag: &str,
    kind: &str,
    optional: bool,
    default: Value,
    order: u32,
) -> Value {
    json!({"name": name, "flag": flag, "kind": kind, "optional": optional,
           "default": default, "order": order})
}

/// cli_spec for `extract.entities_relations`: renders the flat-flag argv a
/// user would type directly (`kg-extract -o kg-protocol …`); the `text` field
/// has no flag — it is fed on stdin when `file` is absent.
fn extract_cli_spec() -> Value {
    json!({
        "subcommand": [],
        "always": ["-o", "kg-protocol"],
        "positionals": [],
        "flags": [
            cli_flag("file", "--file", "string", true, 1),
            cli_flag_default("input_format", "--input-format", "string", true, json!("text"), 2),
            cli_flag_default("engine", "--engine", "string", true, json!("simple"), 3),
            cli_flag_default("backend", "--backend", "string", true, json!("llms"), 4),
            cli_flag_default("agent", "--agent", "string", true, json!("minimaxcc"), 5),
            cli_flag("model", "--model", "string", true, 6),
            cli_flag_default("chunker", "--chunker", "string", true, json!("recursive"), 7),
            cli_flag("schema", "--schema", "string", true, 8),
            cli_flag_default("schema_mode", "--schema-mode", "string", true, json!("open"), 9),
            cli_flag("preset", "--preset", "string", true, 10),
            cli_flag("preset_file", "--preset-file", "string", true, 11),
            cli_flag("lang", "--lang", "string", true, 12),
            cli_flag_default("max_rounds", "--max-rounds", "number", true, json!(1), 13),
            cli_flag_default("merge_strategy", "--merge-strategy", "string", true, json!("keep-existing"), 14),
            cli_flag_default("coref", "--coref", "boolean", true, json!(false), 15),
            cli_flag_default("canonical_direction", "--canonical-direction", "boolean", true, json!(false), 16),
            cli_flag_default("max_concurrency", "--max-concurrency", "number", true, json!(8), 17),
            cli_flag_default("relation_gleaning", "--relation-gleaning", "number", true, json!(0), 18),
            cli_flag("mock_response", "--mock-response", "string", true, 19),
            cli_flag("mock_tool_calls", "--mock-tool-calls", "string", true, 20)
        ]
    })
}

/// cli_spec for the graph-in capabilities: no flat-flag equivalent exists
/// (extraction flags would imply an LLM call), so the template renders the
/// `invoke` form itself — `kg-extract invoke <capability_id> --request -`
/// with the request JSON on stdin (`--request <file>` also works).
fn invoke_cli_spec(capability_id: &str, artifacts: bool) -> Value {
    let mut flags = vec![cli_flag_default(
        "request",
        "--request",
        "string",
        false,
        json!("-"),
        1,
    )];
    if artifacts {
        flags.push(cli_flag("artifacts_dir", "--artifacts-dir", "string", true, 2));
    }
    json!({
        "subcommand": ["invoke", capability_id],
        "always": [],
        "positionals": [],
        "flags": flags
    })
}

fn extract_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [],
        "description": "Extraction request: exactly one of `text` / `file` must be set; every other field mirrors the CLI flag of the same name and falls back to the CLI default when omitted.",
        "properties": {
            "text": str_prop("Source text to extract entities and relations from. Exactly one of `text` / `file` is required. Maps to the CLI's stdin stream (no flag): with cli_spec rendering, pipe this value to the process when `file` is absent."),
            "file": str_prop("Path to a UTF-8 text file to extract from (tilde expanded). Alternative to `text`; exactly one of the two must be set. Renders as `--file`."),
            "input_format": enum_prop(
                "What the input contains.",
                &[
                    ("text", "Plain text, segmented internally per `chunker` (default)."),
                    ("chunks", "Pre-chunked chonkie output (JSON array or JSONL) consumed as-is: the chunking engines (simple/agentic) skip re-chunking and extract per given chunk; the single-shot engines join chunk texts."),
                ],
                "text"),
            "engine": enum_prop(
                "Extraction engine (mechanism driving the model).",
                &[
                    ("simple", "Delimiter-format prompt with multi-gleaning for recall; chunk-aware, high recall (default)."),
                    ("schema-json", "Schema-guided JSON prompt; honours `schema_mode`/`schema`/`preset`; joins chunks into one call."),
                    ("toolcall", "Model calls typed add_entity/add_relation tools; structured by construction, no output parsing; `max_rounds` controls tool-calling rounds."),
                    ("agentic", "Experimental: the whole document runs through one sandboxed multi-turn agent session (slices fed as turns, agent can grep the doc). Ignores `backend`; the provider is chosen by `agent`."),
                ],
                "simple"),
            "backend": enum_prop(
                "Completion backend that turns prompts into model text.",
                &[
                    ("llms", "In-process `llms` crate; requires a build with the `llms-backend` feature (see `available --json`)."),
                    ("agent", "Agent CLI driven over the stream-json protocol (`agent` field picks minimaxcc/glmcc/mimocc or pi-agent); needs the provider API key in the environment."),
                    ("mock", "Deterministic mock replaying `mock_response`/`mock_tool_calls`; for tests and offline demos, always available."),
                ],
                "llms"),
            "agent": str_prop("Agent CLI to use with backend `agent`: `minimaxcc` (default), `glmcc`, `mimocc`, or `pi-agent`. The first three need MINIMAX_API_KEY / GLM_API_KEY / MIMO_API_KEY set respectively; `pi-agent` needs the pi-agent binary on PATH."),
            "model": str_prop("Model name overriding the engine default (e.g. `qwen-max`). Omitted = the backend's built-in default."),
            "chunker": enum_prop(
                "Text segmentation strategy applied before prompting.",
                &[
                    ("recursive", "Recursive structure-aware splitting (default)."),
                    ("char", "Fixed character windows, reproducing the Python extractor 1:1."),
                    ("token", "Tokenizer-based windows."),
                ],
                "recursive"),
            "schema": str_prop("Path to a schema JSON file (entity/relation/attribute types) constraining schema-json/toolcall/agentic. Required when `schema_mode` is `fixed` or `evolving`; ignored under `open`."),
            "schema_mode": enum_prop(
                "How a seed schema constrains extracted types.",
                &[
                    ("open", "No constraint: any type the model emits is accepted (default)."),
                    ("fixed", "Closed world: out-of-schema entities/relations are hard-dropped; requires `schema` (or a preset)."),
                    ("evolving", "Seed from `schema` but allow and record new types; requires `schema` (or a preset)."),
                ],
                "open"),
            "preset": str_prop("Bundled extraction preset (rich template) by key, e.g. `general/concept_graph` or a bare `graph` (resolved under `general/`). Routes the run through the schema-json engine and drives the prompt from the preset's guideline + fields."),
            "preset_file": str_prop("Path to a custom template YAML in the preset format; takes precedence over `preset`."),
            "lang": str_prop("Language to render the preset/template guideline in (e.g. `zh`, `en`). Defaults to the template's first declared language."),
            "max_rounds": int_prop("toolcall engine: maximum tool-calling rounds; 1 = single-round collection (default).", 1, 1),
            "merge_strategy": enum_prop(
                "How duplicate entities (same label) are combined at dedup time.",
                &[
                    ("keep-existing", "Keep the first occurrence (default); later duplicates only contribute citations."),
                    ("keep-incoming", "Replace with the later occurrence (id preserved)."),
                    ("field-union", "Field-wise union: max confidence, longer description, specific type over OTHER, metadata union."),
                    ("llm", "Like field-union but the model synthesises one merged description when both differ (uses the backend)."),
                ],
                "keep-existing"),
            "coref": bool_prop("Cross-chunk entity coreference: also merge surface variants of the same name (case, punctuation, corporate suffixes, near-identical spellings) instead of exact-label duplicates only. Off by default. Also available standalone as the `resolve.coref` capability (kg-document in, kg-document out).", false),
            "canonical_direction": bool_prop("Canonical predicate direction: flip triples on the non-canonical member of a kg-vocab inverse pair (endpoints swapped, predicate → inverse) before dedup, so (A, USES, B) and (B, IS_USED_BY, A) collapse to one edge. Off by default. Also available standalone as `resolve.canonical_direction`.", false),
            "max_concurrency": int_prop("Bound on concurrent backend calls (simple engine per-chunk extraction). 1 = sequential; default 8.", 8, 1),
            "relation_gleaning": int_prop("simple/agentic engines: targeted relation-gleaning rounds re-questioning orphan entities (no relationship) to recover their edges. 0 = off (default).", 0, 0),
            "mock_response": str_prop("Canned completion for backend `mock` (test/demo path), e.g. the simple engine's delimiter format or a JSON object for schema-json."),
            "mock_tool_calls": json!({"type": "string", "description": "Scripted tool calls for backend `mock` with engine `toolcall`: inline JSON `[{\"name\": ..., \"arguments\": {...}}]` (one round) or `[[...],[...]]` (many rounds), or a path to such a JSON file."})
        }
    })
}

fn document_input_properties() -> Value {
    json!({
        "document": json!({"type": "object", "description": "Inline kg.protocol.v1 document ({schema_version, entities, relations, ...}), e.g. the artifact produced by `extract.entities_relations`. Exactly one of `document` / `document_file` is required."}),
        "document_file": str_prop("Path to a kg.protocol.v1 JSON file (tilde expanded). Alternative to inline `document`; exactly one of the two must be set.")
    })
}

fn graph_in_input_schema(desc: &str, extra: Value) -> Value {
    let mut properties = document_input_properties();
    if let (Some(base), Some(extra)) = (properties.as_object_mut(), extra.as_object()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [],
        "description": desc,
        "properties": properties
    })
}

// The manifest builder takes one slot per protocol field; splitting it would
// only obfuscate the 1:1 mapping to the wire shape.
#[allow(clippy::too_many_arguments)]
fn capability(
    id: &str,
    title: &str,
    description: &str,
    side_effects: &[&str],
    input_schema: Value,
    output_mode: &str,
    output_kind: &str,
    cli_spec: Value,
) -> Value {
    json!({
        "capability_id": id,
        "title": title,
        "description": description,
        "side_effects": side_effects,
        "input_schema": input_schema,
        "output": {"mode": output_mode, "kind": output_kind},
        "cli_spec": cli_spec
    })
}

/// The `kg.provider/v1` manifest. `version` is the crate version
/// (`CARGO_PKG_VERSION` at the CLI boundary).
pub fn describe_document(version: &str) -> Value {
    let graph_in = "Exactly one of `document` / `document_file` must be set; the document is a kg.protocol.v1 JSON graph.";
    json!({
        "protocol": PROVIDER_PROTOCOL,
        "protocol_versions": [1],
        "provider": {
            "id": PROVIDER_ID,
            "version": version,
            "description": "Multi-strategy knowledge-graph extraction (simple / schema-json / toolcall / agentic engines) with community detection, GraphRAG-style community summaries, and graph-in/graph-out coref and canonical-direction resolution."
        },
        "capabilities": [
            capability(
                CAP_EXTRACT,
                "Extract entities & relations",
                "Extract a typed knowledge graph (entities + predicate-typed relations) from text and emit it as a kg.protocol.v1 document artifact. Drives an LLM through the chosen engine/backend, so requests leave the machine.",
                &["network", "data_egress"],
                extract_input_schema(),
                "artifact",
                "kg-document",
                extract_cli_spec()),
            capability(
                CAP_DETECT_COMMUNITIES,
                "Detect communities (label propagation)",
                "Label-propagation community detection over a kg.protocol.v1 graph: every relation becomes one weight-1.0 edge and parallel edges are summed, so relation multiplicity acts as edge weight. Pure local computation, deterministic output ({num_communities, quality: null, communities}). Requires a build with the `community` feature (see `available --json`).",
                &[],
                graph_in_input_schema(graph_in, json!({})),
                "result-json",
                "communities",
                invoke_cli_spec(CAP_DETECT_COMMUNITIES, false)),
            capability(
                CAP_DETECT_HIERARCHY,
                "Detect community hierarchy (hierarchical Leiden)",
                "Hierarchical Leiden levels (coarse → fine) over a kg.protocol.v1 graph, each level carrying its modularity quality score; fixed-seed detection and sorted member ids make the output deterministic. Pure local computation. Requires a build with the `community-leiden` feature (see `available --json`).",
                &[],
                graph_in_input_schema(graph_in, json!({})),
                "result-json",
                "communities",
                invoke_cli_spec(CAP_DETECT_HIERARCHY, false)),
            capability(
                CAP_SUMMARIZE,
                "Summarize communities (GraphRAG-style reports)",
                "One backend completion per community (per level when `hierarchy` is true) generating a {name, summary} report merged into the communities JSON. Calls run with bounded concurrency and the output is deterministic regardless of completion order; a failed community degrades to null name/summary instead of failing the run. Requires the `community` feature (`community-leiden` when `hierarchy` is true).",
                &["network", "data_egress"],
                graph_in_input_schema(graph_in, json!({
                    "hierarchy": bool_prop("Summarize hierarchical-Leiden levels (coarse → fine) instead of the flat label-propagation partition. Requires the `community-leiden` feature.", false),
                    "backend": enum_prop(
                        "Completion backend used for the per-community summary calls.",
                        &[
                            ("llms", "In-process `llms` crate; requires a build with the `llms-backend` feature."),
                            ("agent", "Agent CLI over stream-json (`agent` picks minimaxcc/glmcc/mimocc or pi-agent); needs the provider API key in the environment."),
                            ("mock", "Deterministic mock replaying `mock_response`; always available."),
                        ],
                        "llms"),
                    "agent": str_prop("Agent CLI to use with backend `agent`: `minimaxcc` (default), `glmcc`, `mimocc`, or `pi-agent`."),
                    "model": str_prop("Model name for the summary calls; omitted = the backend's built-in default."),
                    "max_concurrency": int_prop("Bound on concurrent summary calls; 1 = sequential; default 8. Output is identical for any value.", 8, 1),
                    "mock_response": str_prop("Canned completion for backend `mock`, e.g. {\"name\":\"...\",\"summary\":\"...\"} applied to every community."),
                    "mock_tool_calls": json!({"type": "string", "description": "Scripted tool-call rounds for backend `mock` (accepted for symmetry with extract; unused by the summary path)."})
                })),
                "result-json",
                "communities",
                invoke_cli_spec(CAP_SUMMARIZE, false)),
            capability(
                CAP_RESOLVE_COREF,
                "Resolve entity coreference",
                "Fuzzy coreference resolution over a kg.protocol.v1 graph: merge surface variants of the same entity (case, punctuation, corporate suffixes, near-identical spellings, type-compatible only) and remap relations onto the canonical entity id. Pure local computation; the same pass the extract `coref` option runs, exposed graph-in/graph-out so it can post-process any kg.protocol.v1 document.",
                &[],
                graph_in_input_schema(graph_in, json!({
                    "merge_strategy": enum_prop(
                        "How a recognised duplicate is folded into the canonical entity.",
                        &[
                            ("keep-existing", "Keep the first occurrence (default); duplicates contribute citations only."),
                            ("keep-incoming", "Replace with the later occurrence (id preserved)."),
                            ("field-union", "Field-wise union: max confidence, longer description, specific type over OTHER, metadata union."),
                            ("llm", "Degrades to field-union here (no backend in this capability)."),
                        ],
                        "keep-existing")
                })),
                "artifact",
                "kg-document",
                invoke_cli_spec(CAP_RESOLVE_COREF, true)),
            capability(
                CAP_RESOLVE_DIRECTION,
                "Normalize canonical predicate direction",
                "Flip triples on the non-canonical member of a kg-vocab inverse pair (endpoints swapped, predicate → inverse) and dedup, so direction variants like (A, USES, B) vs (B, IS_USED_BY, A) collapse to one edge with unioned citations; unpaired predicates are untouched. Pure local computation; the same pass the extract `canonical_direction` option runs, exposed graph-in/graph-out.",
                &[],
                graph_in_input_schema(graph_in, json!({})),
                "artifact",
                "kg-document",
                invoke_cli_spec(CAP_RESOLVE_DIRECTION, true))
        ]
    })
}

// ---------------------------------------------------------------------------
// available
// ---------------------------------------------------------------------------

fn binary_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file() && is_executable(&candidate)
    })
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Read-only availability probe: which backends/features this binary can
/// actually use. Never performs network calls — agent backends reuse the
/// backend's own provider-env resolution (a missing API key env var = missing),
/// pi-agent probes PATH, `llms` and the community detectors probe compiled
/// features. `available` is true only when *everything* is ready; partial
/// readiness is expressed in `ready` / `missing`.
pub fn available_report() -> Value {
    let mut ready: Vec<Value> = Vec::new();
    let mut missing: Vec<Value> = Vec::new();
    let mut check = |name: &str, kind: &str, ok: bool| {
        if ok {
            ready.push(json!({"name": name, "kind": kind}));
        } else {
            missing.push(json!({"name": name, "kind": kind}));
        }
    };

    check("backend:mock", "backend", true);
    check("backend:llms", "backend", cfg!(feature = "llms-backend"));
    // Reuses the backend's own provider-env resolution: a known agent whose
    // API-key env var is unset resolves to Err, i.e. missing. Read-only.
    for agent in ["minimaxcc", "glmcc", "mimocc"] {
        check(
            &format!("backend:agent:{agent}"),
            "backend",
            SdkAgentBackend::for_agent(agent).is_ok(),
        );
    }
    check(
        "backend:agent:pi-agent",
        "backend",
        PiAgentBackend::accepts("pi-agent") && binary_on_path("pi-agent"),
    );
    check("feature:community", "feature", cfg!(feature = "community"));
    check(
        "feature:community-leiden",
        "feature",
        cfg!(feature = "community-leiden"),
    );

    // No local weight cache to report: `cache_dir` is omitted rather than
    // null (the hub's available-v1 schema types it as a string).
    json!({
        "available": missing.is_empty(),
        "ready": ready,
        "missing": missing
    })
}

// ---------------------------------------------------------------------------
// invoke
// ---------------------------------------------------------------------------

/// Stable machine-readable error codes for the `kg.execution/v1` envelope.
pub mod error_code {
    /// Request JSON malformed, fails the capability's input contract
    /// (missing/unknown/conflicting fields, unreadable files, bad enums).
    pub const INVALID_REQUEST: &str = "invalid_request";
    /// The capability id is not one this provider exposes.
    pub const UNKNOWN_CAPABILITY: &str = "unknown_capability";
    /// A required backend or compiled feature is unavailable (e.g. `llms`
    /// without the `llms-backend` feature, community detection without the
    /// `community` feature, a missing API key env var).
    pub const BACKEND_UNAVAILABLE: &str = "backend_unavailable";
    /// The extraction engine (or the summary calls around it) failed.
    pub const EXTRACTION_FAILED: &str = "extraction_failed";
}

/// Outcome of one [`invoke`] call: the envelope to print plus the process
/// success flag (`false` ⇒ the CLI exits non-zero after printing).
pub struct InvokeOutcome {
    pub envelope: Value,
    pub ok: bool,
}

fn ok_envelope(
    capability_id: &str,
    result: Value,
    artifacts: Vec<Value>,
    diagnostics: Vec<Value>,
) -> InvokeOutcome {
    InvokeOutcome {
        envelope: json!({
            "protocol": EXECUTION_PROTOCOL,
            "capability_id": capability_id,
            "provider": PROVIDER_ID,
            "status": "ok",
            "result": result,
            "artifacts": artifacts,
            "diagnostics": diagnostics
        }),
        ok: true,
    }
}

fn error_envelope(capability_id: &str, code: &str, message: String) -> InvokeOutcome {
    InvokeOutcome {
        envelope: json!({
            "protocol": EXECUTION_PROTOCOL,
            "capability_id": capability_id,
            "provider": PROVIDER_ID,
            "status": "error",
            "result": null,
            "artifacts": [],
            "diagnostics": [],
            "error": {"code": code, "message": message}
        }),
        ok: false,
    }
}

struct InvokeError {
    code: &'static str,
    message: String,
}

impl InvokeError {
    fn invalid(message: impl Into<String>) -> Self {
        InvokeError {
            code: error_code::INVALID_REQUEST,
            message: message.into(),
        }
    }
    fn backend(message: impl Into<String>) -> Self {
        InvokeError {
            code: error_code::BACKEND_UNAVAILABLE,
            message: message.into(),
        }
    }
    fn extraction(message: impl Into<String>) -> Self {
        InvokeError {
            code: error_code::EXTRACTION_FAILED,
            message: message.into(),
        }
    }
}

fn diagnostic(severity: &str, message: impl Into<String>) -> Value {
    json!({"severity": severity, "message": message.into()})
}

/// Run one capability from a raw JSON request string. `artifacts_dir` is where
/// artifact-producing capabilities write their output files (a fresh temp dir
/// when `None`). The returned envelope is the *only* thing that belongs on the
/// caller's stdout.
pub async fn invoke(
    capability_id: &str,
    request_raw: &str,
    artifacts_dir: Option<&Path>,
) -> InvokeOutcome {
    match invoke_inner(capability_id, request_raw, artifacts_dir).await {
        Ok(outcome) => outcome,
        Err(e) => error_envelope(capability_id, e.code, e.message),
    }
}

async fn invoke_inner(
    capability_id: &str,
    request_raw: &str,
    artifacts_dir: Option<&Path>,
) -> Result<InvokeOutcome, InvokeError> {
    let mut request: Value = serde_json::from_str(request_raw)
        .map_err(|e| InvokeError::invalid(format!("request is not valid JSON: {e}")))?;
    // The hub speaks the wrapped form {"capability_id", "input"} (kg-acme
    // spec 01 §3); the bare input object stays accepted for direct CLI use.
    if let Some(obj) = request.as_object() {
        if obj.contains_key("capability_id") && obj.contains_key("input") {
            let wrapped = obj["capability_id"].as_str().unwrap_or_default();
            if wrapped != capability_id {
                return Err(InvokeError::invalid(format!(
                    "request capability_id '{wrapped}' does not match the invoked capability '{capability_id}'"
                )));
            }
            request = obj["input"].clone();
        }
    }
    match capability_id {
        CAP_EXTRACT => invoke_extract(&request, artifacts_dir).await,
        CAP_DETECT_COMMUNITIES => invoke_detect_communities(&request),
        CAP_DETECT_HIERARCHY => invoke_detect_hierarchy(&request),
        CAP_SUMMARIZE => invoke_summarize(&request).await,
        CAP_RESOLVE_COREF => invoke_resolve_coref(&request, artifacts_dir),
        CAP_RESOLVE_DIRECTION => invoke_resolve_direction(&request, artifacts_dir),
        other => Err(InvokeError {
            code: error_code::UNKNOWN_CAPABILITY,
            message: format!(
                "unknown capability '{other}' (this provider exposes: {})",
                CAPABILITY_IDS.join(", ")
            ),
        }),
    }
}

// ---- request shapes -------------------------------------------------------

/// `extract.entities_relations` request; every field mirrors the CLI flag of
/// the same name (see [`extract_input_schema`]).
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ExtractRequest {
    text: Option<String>,
    file: Option<String>,
    input_format: Option<String>,
    engine: Option<String>,
    backend: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    chunker: Option<String>,
    schema: Option<String>,
    schema_mode: Option<String>,
    preset: Option<String>,
    preset_file: Option<String>,
    lang: Option<String>,
    max_rounds: Option<usize>,
    merge_strategy: Option<String>,
    coref: Option<bool>,
    canonical_direction: Option<bool>,
    max_concurrency: Option<usize>,
    relation_gleaning: Option<usize>,
    mock_response: Option<String>,
    mock_tool_calls: Option<String>,
}

/// Shared graph-in request: inline document or a file carrying one.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DocumentRequest {
    document: Option<Value>,
    document_file: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SummarizeRequest {
    document: Option<Value>,
    document_file: Option<String>,
    hierarchy: Option<bool>,
    backend: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    max_concurrency: Option<usize>,
    mock_response: Option<String>,
    mock_tool_calls: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CorefRequest {
    document: Option<Value>,
    document_file: Option<String>,
    merge_strategy: Option<String>,
}

fn parse_request<T: serde::de::DeserializeOwned>(
    request: &Value,
    what: &str,
) -> Result<T, InvokeError> {
    serde_json::from_value::<T>(request.clone())
        .map_err(|e| InvokeError::invalid(format!("invalid {what} request: {e}")))
}

// ---- enum parsing helpers (invalid enum = invalid_request, listing values) --

fn parse_choice(
    field: &str,
    value: Option<&str>,
    default: &str,
    valid: &[&str],
) -> Result<String, InvokeError> {
    let v = value.unwrap_or(default);
    if valid.contains(&v) {
        Ok(v.to_string())
    } else {
        Err(InvokeError::invalid(format!(
            "unknown {field} '{v}' (expected one of: {})",
            valid.join(", ")
        )))
    }
}

fn parse_merge_strategy(value: Option<&str>) -> Result<MergeStrategy, InvokeError> {
    match parse_choice(
        "merge_strategy",
        value,
        "keep-existing",
        &["keep-existing", "keep-incoming", "field-union", "llm"],
    )?
    .as_str()
    {
        "keep-existing" => Ok(MergeStrategy::KeepExisting),
        "keep-incoming" => Ok(MergeStrategy::KeepIncoming),
        "field-union" => Ok(MergeStrategy::FieldUnion),
        _ => Ok(MergeStrategy::Llm),
    }
}

// ---- artifacts ------------------------------------------------------------

/// Write `contents` as an artifact file and return its envelope entry
/// (`path` + `kind` + `checksum` = `sha256:<hex>`).
fn write_artifact(
    artifacts_dir: Option<&Path>,
    filename: &str,
    kind: &str,
    contents: &str,
) -> Result<Value, InvokeError> {
    let dir = match artifacts_dir {
        Some(d) => expand_tilde(&d.to_string_lossy()),
        None => std::env::temp_dir().join(format!("kg-extract-invoke-{}", nanoid::nanoid!())),
    };
    std::fs::create_dir_all(&dir).map_err(|e| {
        InvokeError::extraction(format!("creating artifacts dir {}: {e}", dir.display()))
    })?;
    let path = dir.join(filename);
    std::fs::write(&path, contents).map_err(|e| {
        InvokeError::extraction(format!("writing artifact {}: {e}", path.display()))
    })?;
    let checksum = format!("sha256:{:x}", Sha256::digest(contents.as_bytes()));
    Ok(json!({
        "path": path.to_string_lossy(),
        "kind": kind,
        "checksum": checksum
    }))
}

// ---- graph-in helpers -----------------------------------------------------

/// Load the request's kg.protocol.v1 document into the extractor-domain graph.
/// Import drops (dangling relations, range-less evidence) become diagnostics.
fn load_document(
    document: &Option<Value>,
    document_file: &Option<String>,
) -> Result<(KnowledgeGraph, Vec<Value>), InvokeError> {
    let raw: Value = match (document, document_file) {
        (Some(doc), None) => doc.clone(),
        (None, Some(path)) => {
            let path = expand_tilde(path);
            let body = std::fs::read_to_string(&path).map_err(|e| {
                InvokeError::invalid(format!("reading document_file {}: {e}", path.display()))
            })?;
            serde_json::from_str(&body).map_err(|e| {
                InvokeError::invalid(format!(
                    "document_file {} is not valid JSON: {e}",
                    path.display()
                ))
            })?
        }
        (Some(_), Some(_)) => {
            return Err(InvokeError::invalid(
                "provide exactly one of `document` / `document_file`, not both",
            ))
        }
        (None, None) => {
            return Err(InvokeError::invalid(
                "provide exactly one of `document` / `document_file`",
            ))
        }
    };
    let doc: core_types_rs::KgDocument = serde_json::from_value(raw)
        .map_err(|e| InvokeError::invalid(format!("document is not a kg.protocol.v1 document: {e}")))?;
    let mut diagnostics = Vec::new();
    if doc.schema_version != core_types_rs::KG_PROTOCOL_VERSION {
        diagnostics.push(diagnostic(
            "warning",
            format!(
                "document schema_version '{}' differs from '{}'; importing anyway",
                doc.schema_version,
                core_types_rs::KG_PROTOCOL_VERSION
            ),
        ));
    }
    let (kg, report) = KnowledgeGraph::from_kg_document(&doc);
    if report.dangling_relations > 0 {
        diagnostics.push(diagnostic(
            "warning",
            format!(
                "dropped {} relation(s) whose endpoint is not a declared entity",
                report.dangling_relations
            ),
        ));
    }
    if report.range_less_evidence > 0 {
        diagnostics.push(diagnostic(
            "warning",
            format!(
                "dropped {} evidence entr{} without a source range (only ranged evidence round-trips)",
                report.range_less_evidence,
                if report.range_less_evidence == 1 { "y" } else { "ies" }
            ),
        ));
    }
    Ok((kg, diagnostics))
}

/// Serialize a graph as a kg.protocol.v1 artifact plus entity/relation counts
/// for the envelope's `result`.
fn kg_document_artifact(
    kg: &KnowledgeGraph,
    artifacts_dir: Option<&Path>,
) -> Result<(Value, Value), InvokeError> {
    let doc = kg.to_kg_document();
    let body = serde_json::to_string_pretty(&doc)
        .map_err(|e| InvokeError::extraction(format!("serializing kg-document: {e}")))?;
    let artifact = write_artifact(artifacts_dir, "kg-document.json", "kg-document", &body)?;
    let result = json!({
        "num_entities": kg.entities.len(),
        "num_relations": kg.triples.len()
    });
    Ok((result, artifact))
}

// ---- capability runners ----------------------------------------------------

async fn invoke_extract(
    request: &Value,
    artifacts_dir: Option<&Path>,
) -> Result<InvokeOutcome, InvokeError> {
    let req: ExtractRequest = parse_request(request, CAP_EXTRACT)?;
    let mut diagnostics: Vec<Value> = Vec::new();

    let text = match (&req.text, &req.file) {
        (Some(text), None) => text.clone(),
        (None, Some(path)) => {
            let path = expand_tilde(path);
            std::fs::read_to_string(&path).map_err(|e| {
                InvokeError::invalid(format!("reading file {}: {e}", path.display()))
            })?
        }
        (Some(_), Some(_)) => {
            return Err(InvokeError::invalid(
                "provide exactly one of `text` / `file`, not both",
            ))
        }
        (None, None) => {
            return Err(InvokeError::invalid(
                "provide exactly one of `text` / `file`",
            ))
        }
    };
    if text.trim().is_empty() {
        return Err(InvokeError::invalid("input text is empty"));
    }

    let engine = parse_choice(
        "engine",
        req.engine.as_deref(),
        "simple",
        &["simple", "schema-json", "toolcall", "agentic"],
    )?;
    let backend_name = parse_choice("backend", req.backend.as_deref(), "llms", &["llms", "agent", "mock"])?;
    let agent = req.agent.clone().unwrap_or_else(|| "minimaxcc".to_string());
    let chunker = match parse_choice("chunker", req.chunker.as_deref(), "recursive", &["char", "recursive", "token"])?.as_str() {
        "char" => ChunkStrategy::Char,
        "token" => ChunkStrategy::Token,
        _ => ChunkStrategy::Recursive,
    };
    let schema_mode = match parse_choice("schema_mode", req.schema_mode.as_deref(), "open", &["open", "fixed", "evolving"])?.as_str() {
        "fixed" => SchemaMode::Fixed,
        "evolving" => SchemaMode::Evolving,
        _ => SchemaMode::Open,
    };
    let input_format = parse_choice("input_format", req.input_format.as_deref(), "text", &["text", "chunks"])?;
    let merge_strategy = parse_merge_strategy(req.merge_strategy.as_deref())?;
    let coref = req.coref.unwrap_or(false);
    let canonical_direction = req.canonical_direction.unwrap_or(false);
    let max_concurrency = req.max_concurrency.unwrap_or(8);
    let relation_gleaning = req.relation_gleaning.unwrap_or(0);
    let max_rounds = req.max_rounds.unwrap_or(1);

    // A preset/template is only honoured by the schema-json engine; loading
    // one routes the run there (mirrors the CLI).
    let template: Option<TemplateCfg> = if let Some(path) = &req.preset_file {
        Some(
            TemplateCfg::from_yaml_file(expand_tilde(path))
                .map_err(|e| InvokeError::invalid(format!("loading preset_file {path}: {e}")))?,
        )
    } else if let Some(name) = &req.preset {
        Some(gallery::get(name).ok_or_else(|| {
            InvokeError::invalid(format!(
                "unknown preset '{name}' ({} presets available)",
                gallery::list().len()
            ))
        })?)
    } else {
        None
    };
    let engine = if template.is_some() && engine != "schema-json" {
        diagnostics.push(diagnostic(
            "info",
            "preset/preset_file routes through the schema-json engine",
        ));
        "schema-json".to_string()
    } else {
        engine
    };

    // Pre-chunked input parses up front so a malformed file fails before any
    // backend is constructed.
    let prechunked = if input_format == "chunks" {
        Some(
            crate::chunking::parse_prechunked(&text)
                .map_err(|e| InvokeError::invalid(format!("parsing chunks input: {e}")))?,
        )
    } else {
        None
    };
    let source_doc: Option<String> = match &prechunked {
        Some(p) => p.source.clone(),
        None => req.file.clone(),
    };

    let load_schema = || -> Result<Schema, InvokeError> {
        let path = req.schema.clone().expect("caller checked presence");
        Schema::from_json_file(expand_tilde(&path))
            .map_err(|e| InvokeError::invalid(format!("loading schema {path}: {e}")))
    };

    // Mirrors the CLI's engine dispatch (kg-extract.rs main): agentic drives
    // the SDK client itself and bypasses the backend; the other engines share
    // one backend construction.
    let extractor: Box<dyn Extractor + Send + Sync> = if engine == "agentic" {
        let mut c = AgenticExtractor::default_config();
        c.chunker = chunker;
        c.source_doc = source_doc;
        c.spec.canonical_direction = canonical_direction;
        if let Some(m) = &req.model {
            c.model_name = m.clone();
        }
        if req.schema.is_some() {
            c.spec.schema = load_schema()?;
        }
        Box::new(
            AgenticExtractor::with_config(&agent, c)
                .schema_mode(schema_mode)
                .relation_gleanings(relation_gleaning),
        )
    } else {
        let backend = make_backend(
            &backend_name,
            &agent,
            req.mock_response.as_deref(),
            req.mock_tool_calls.as_deref(),
        )
        .map_err(|e| InvokeError::backend(e.to_string()))?;
        let coref_mode = if coref {
            CorefMode::Fuzzy
        } else {
            CorefMode::Off
        };
        match engine.as_str() {
            "simple" => {
                let mut c = SimpleExtractor::default_config();
                c.chunker = chunker;
                c.source_doc = source_doc;
                c.max_concurrency = max_concurrency;
                c.spec.merge_strategy = merge_strategy;
                c.spec.coref = coref_mode;
                c.spec.canonical_direction = canonical_direction;
                if let Some(m) = &req.model {
                    c.model_name = m.clone();
                }
                Box::new(
                    SimpleExtractor::with_config(backend, c).relation_gleanings(relation_gleaning),
                )
            }
            "schema-json" => {
                let mut c = SchemaJsonExtractor::default_config();
                c.chunker = chunker;
                c.source_doc = source_doc;
                c.spec.merge_strategy = merge_strategy;
                c.spec.canonical_direction = canonical_direction;
                if let Some(m) = &req.model {
                    c.model_name = m.clone();
                }
                if req.schema.is_some() {
                    c.spec.schema = load_schema()?;
                }
                if let Some(tpl) = template {
                    c.spec.language = req.lang.clone();
                    c.spec.template = Some(tpl);
                }
                Box::new(SchemaJsonExtractor::with_config(backend, c).schema_mode(schema_mode))
            }
            "toolcall" => {
                let mut c = ToolCallExtractor::default_config();
                c.chunker = chunker;
                c.source_doc = source_doc;
                c.spec.merge_strategy = merge_strategy;
                c.spec.coref = coref_mode;
                c.spec.canonical_direction = canonical_direction;
                if let Some(m) = &req.model {
                    c.model_name = m.clone();
                }
                if req.schema.is_some() {
                    c.spec.schema = load_schema()?;
                }
                Box::new(
                    ToolCallExtractor::with_config(backend, c)
                        .schema_mode(schema_mode)
                        .max_rounds(max_rounds),
                )
            }
            other => unreachable!("engine validated above: {other}"),
        }
    };

    let mut response = match &prechunked {
        Some(p) => extractor.extract_prechunked(&p.segments).await,
        None => extractor.extract(&text).await,
    }
    .map_err(|e| InvokeError::extraction(e.to_string()))?;

    if response.annotate_type_normalization() {
        if let Some(c) = response
            .metadata
            .get("type_normalization")
            .and_then(|v| v.get("counts"))
        {
            diagnostics.push(diagnostic("info", format!("type normalization: {c}")));
        }
    }

    let stats = response.get_stats();
    let (mut result, artifact) = kg_document_artifact(&response.knowledge_graph, artifacts_dir)?;
    if let (Some(r), Some(s)) = (result.as_object_mut(), stats.as_object()) {
        for (k, v) in s {
            r.insert(k.clone(), v.clone());
        }
    }
    result
        .as_object_mut()
        .expect("stats merge above")
        .insert("document".into(), json!(artifact["path"]));
    Ok(ok_envelope(CAP_EXTRACT, result, vec![artifact], diagnostics))
}

fn invoke_detect_communities(request: &Value) -> Result<InvokeOutcome, InvokeError> {
    let req: DocumentRequest = parse_request(request, CAP_DETECT_COMMUNITIES)?;
    let (kg, diagnostics) = load_document(&req.document, &req.document_file)?;
    #[cfg(feature = "community")]
    {
        let result = crate::community::communities_json(&kg);
        Ok(ok_envelope(CAP_DETECT_COMMUNITIES, result, vec![], diagnostics))
    }
    #[cfg(not(feature = "community"))]
    {
        let _ = (kg, diagnostics);
        Err(InvokeError::backend(
            "the `detect.communities` capability requires building with --features community",
        ))
    }
}

fn invoke_detect_hierarchy(request: &Value) -> Result<InvokeOutcome, InvokeError> {
    let req: DocumentRequest = parse_request(request, CAP_DETECT_HIERARCHY)?;
    let (kg, diagnostics) = load_document(&req.document, &req.document_file)?;
    #[cfg(feature = "community-leiden")]
    {
        let result = crate::community::hierarchy_json(&kg);
        Ok(ok_envelope(CAP_DETECT_HIERARCHY, result, vec![], diagnostics))
    }
    #[cfg(not(feature = "community-leiden"))]
    {
        let _ = (kg, diagnostics);
        Err(InvokeError::backend(
            "the `detect.communities_hierarchy` capability requires building with --features community-leiden",
        ))
    }
}

async fn invoke_summarize(request: &Value) -> Result<InvokeOutcome, InvokeError> {
    let req: SummarizeRequest = parse_request(request, CAP_SUMMARIZE)?;
    let (kg, diagnostics) = load_document(&req.document, &req.document_file)?;
    #[cfg(feature = "community")]
    {
        let backend_name = parse_choice(
            "backend",
            req.backend.as_deref(),
            "llms",
            &["llms", "agent", "mock"],
        )?;
        let agent = req.agent.clone().unwrap_or_else(|| "minimaxcc".to_string());
        let backend = make_backend(
            &backend_name,
            &agent,
            req.mock_response.as_deref(),
            req.mock_tool_calls.as_deref(),
        )
        .map_err(|e| InvokeError::backend(e.to_string()))?;
        let options = crate::backend::CompletionOptions {
            model: req
                .model
                .clone()
                .unwrap_or_else(|| crate::backend::CompletionOptions::default().model),
            max_tokens: crate::community::SUMMARY_MAX_TOKENS,
            ..crate::backend::CompletionOptions::default()
        };
        let max_concurrency = req.max_concurrency.unwrap_or(8);
        let result = if req.hierarchy.unwrap_or(false) {
            #[cfg(feature = "community-leiden")]
            {
                crate::community::hierarchy_json_with_summaries(
                    &kg,
                    &backend,
                    &options,
                    max_concurrency,
                )
                .await
            }
            #[cfg(not(feature = "community-leiden"))]
            {
                return Err(InvokeError::backend(
                    "hierarchy summaries require building with --features community-leiden",
                ));
            }
        } else {
            crate::community::communities_json_with_summaries(
                &kg,
                &backend,
                &options,
                max_concurrency,
            )
            .await
        };
        Ok(ok_envelope(CAP_SUMMARIZE, result, vec![], diagnostics))
    }
    #[cfg(not(feature = "community"))]
    {
        let _ = (kg, diagnostics);
        Err(InvokeError::backend(
            "the `summarize.communities` capability requires building with --features community",
        ))
    }
}

fn invoke_resolve_coref(
    request: &Value,
    artifacts_dir: Option<&Path>,
) -> Result<InvokeOutcome, InvokeError> {
    let req: CorefRequest = parse_request(request, CAP_RESOLVE_COREF)?;
    let (kg, mut diagnostics) = load_document(&req.document, &req.document_file)?;
    let strategy = parse_merge_strategy(req.merge_strategy.as_deref())?;
    let before = (kg.entities.len(), kg.triples.len());
    // Folding the whole graph into an empty one with fuzzy recognition collapses
    // its own surface variants and remaps triples onto the canonical ids.
    let merged = crate::merger::merge_with_deduplication_strategy_coref(
        KnowledgeGraph::new(),
        kg,
        strategy,
        CorefMode::Fuzzy,
    );
    diagnostics.push(diagnostic(
        "info",
        format!(
            "coref: {} entities / {} relations before, {} / {} after",
            before.0,
            before.1,
            merged.entities.len(),
            merged.triples.len()
        ),
    ));
    let (result, artifact) = kg_document_artifact(&merged, artifacts_dir)?;
    Ok(ok_envelope(CAP_RESOLVE_COREF, result, vec![artifact], diagnostics))
}

fn invoke_resolve_direction(
    request: &Value,
    artifacts_dir: Option<&Path>,
) -> Result<InvokeOutcome, InvokeError> {
    let req: DocumentRequest = parse_request(request, CAP_RESOLVE_DIRECTION)?;
    let (mut kg, mut diagnostics) = load_document(&req.document, &req.document_file)?;
    let before = kg.triples.len();
    crate::merger::normalize_direction(&mut kg);
    // Re-fold so direction variants that now share one dedup key collapse with
    // unioned citations (the merge-stage behaviour this normalisation feeds).
    let merged = crate::merger::merge_with_deduplication_strategy_coref(
        KnowledgeGraph::new(),
        kg,
        MergeStrategy::KeepExisting,
        CorefMode::Off,
    );
    diagnostics.push(diagnostic(
        "info",
        format!(
            "canonical direction: {} relations before, {} after",
            before,
            merged.triples.len()
        ),
    ));
    let (result, artifact) = kg_document_artifact(&merged, artifacts_dir)?;
    Ok(ok_envelope(
        CAP_RESOLVE_DIRECTION,
        result,
        vec![artifact],
        diagnostics,
    ))
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
}
