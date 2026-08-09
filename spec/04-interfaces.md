# 04 — Interfaces

## 5. Architecture / Layering

```
                 ┌─────────── CLI (kg-extract) ───────────┐
                 │   flags → config merge → engine dispatch│
                 └──────────┬───────────────────┬─────────┘
                            │                   │
        ┌──── Extractor trait ────┐         KgStore (MCP)
        │  extract / extract_prechunk │     (load-modify-save, no LLM)
        └──┬───────┬──────┬─────┬───┘
        Simple  SchemaJson Toolcall Agentic
           │        │         │        │
        chunking   parser  tool-specs  SDK sandbox
           │        │         │        │
        ┌──┴── Backend trait ───┴──┐   (Agentic bypasses Backend)
        complete / complete_with_tools / open_session
           │
     LlmsBackend | SdkAgentBackend | PiAgentBackend | MockBackend
```

Responsibility boundaries (not file layout):

- **Extractor trait**: `extract(text)` and `extract_prechunked(chunks)` → one
  `ExtractionResponse`.
- **Backend trait**: turns a message list into assistant text; optionally
  supports tool calling and a native multi-turn session. Backends without a
  native session fall back to `ReplaySession` (replays the whole history each
  turn).
- **GraphBuilder** (shared): deterministic entity id, name-based relation
  resolution with dangling drop, attribute application. Each engine supplies
  its own already-parsed types.
- **Merger / citation**: fold per-chunk graphs, recognize duplicates,
  compute and union provenance.

## 9.1 Extractor interface (library)

```
trait Extractor:
  async extract(text) → Result<ExtractionResponse>
  async extract_prechunked(chunks) → Result<ExtractionResponse>
       // default: join chunk texts with "\n\n", then extract
```

Construction takes a backend (Arc) and a config; a declarative
`ExtractionSpec` runs through either schema-json or toolcall via
`with_spec`. [T] (`schema_json::one_spec_runs_through_both_engines`)

## 9.2 Backend interface

```
trait LlmBackend:
  async complete(messages, options) → Result<string>
  async complete_prompt(prompt, options) → Result<string>        // default = complete([user prompt])
  fn supports_tools() → bool                                     // default false
  async complete_with_tools(messages, tools, options) → Result<ToolChatResponse>  // default: error
  async open_session(system, options) → Result<Option<ChatSession>>             // default None → replay

trait ChatSession:
  async send(prompt) → Result<string>
  async finish() → Result<()>                                   // default no-op
```

`CompletionOptions = { model, temperature, max_tokens }`. Backends include an
in-process LLM backend (feature `llms-backend`), an agent-CLI backend
(`minimaxcc`/`glmcc`/`mimocc` over stream-json), a `pi-agent` backend, and a
deterministic `MockBackend` for tests.

## 9.3 CLI (`kg-extract`)

Settings precedence (highest first): (1) explicit flag, (2) config file
(`--config` or `~/.kg-extract/config.json`), (3) built-in default. A presence
flag (`--coref`, `--canonical-direction`) wins over the config.
`--list-presets` prints the gallery and
exits.

```abnf
engine      = "simple" / "schema-json" / "toolcall" / "agentic"
backend     = "llms" / "agent" / "mock"
chunker     = "char" / "recursive" / "token"
schema-mode = "open" / "fixed" / "evolving"
merge-strat = "keep-existing" / "keep-incoming" / "field-union" / "llm"
input-fmt   = "text" / "chunks"
out-fmt     = "json" / "jsonl" / "kg-protocol" / "node-link"
            / "ladybug-import" / "communities" / "communities-hierarchy"
            / "mermaid" / "stats"
```

Key flag contracts:

| Flag | Contract |
|------|----------|
| `-e/--engine` | selects engine; `-e agentic` ignores `--backend` (drives the SDK directly) |
| `--schema-mode fixed\|evolving` | **MUST** be paired with `--schema` (a non-empty schema file) unless `--preset`/`--preset-file` is set |
| `--preset`/`--preset-file` | routes through schema-json; forces `-e schema-json` (emits a note otherwise) |
| `--max-rounds` | toolcall rounds (1 = single-round, default) |
| `--relation-gleaning N` | simple/agentic rescue rounds (0 = off) |
| `-F/--input-format chunks` | chonkie chunk JSON/JSONL consumed as-is |
| `--coref` | fuzzy cross-chunk coreference |
| `--canonical-direction` | flip triples on the non-canonical member of an inverse pair (endpoints swapped, predicate → inverse) at the merge stage, so direction variants like `(A, USES, B)` vs `(B, IS_USED_BY, A)` dedup to one edge; off by default (spec 8.3.1); a presence flag like `--coref` — CLI wins, else the `canonical_direction` config key |
| `--merge-strategy` | how label-duplicates fold |
| `--max-concurrency N` | bound on concurrent backend calls: Simple per-chunk extraction **and** `--community-summaries` per-community calls (default 8; 1 = sequential) |
| `--community-summaries` | with `-o communities` / `-o communities-hierarchy`: one backend completion per community (per level, in hierarchy mode) generating a `{name, summary}` report merged into the community JSON; calls run with bounded concurrency (`--max-concurrency`) and the output is deterministic regardless of completion order; a failed/unparseable community degrades to null fields with a stderr warning; other output formats emit a note and ignore the flag |

## 9.4 Output formats

| Format | Shape |
|--------|-------|
| `json` *(default)* | `{entities: {id: Entity.to_dict}, triples: [Triple.to_dict], metadata}` |
| `jsonl` | one line per record: `{kind:"entity"\|"triple", data: …}` |
| `kg-protocol` | portable `KgDocument` (`schema_version`, `entities`, `relations`, `evidence`); relations reference entity ids; citations → first-class evidence ranges |
| `node-link` | `{directed:true, nodes:[{id,label,…}], links:[{source,target,type,…}]}` — `source`/`target` reference node `id`s; no RDF `subject`/`object` keys leak |
| `ladybug-import` | generic `KgEntity` node table + one relationship table per predicate; metadata stored as JSON strings |
| `communities` | `{num_communities, quality, communities: {"0": [entity_id, …], …}}` — label-propagation community detection over the multiplicity-weighted graph; community keys ascend from `"0"`, member ids sorted. `quality` is the detector-reported score (`null` — label propagation reports none). With `--community-summaries`, each community renders as `{"members": [entity_id, …], "name": …, "summary": …}` (`null` name/summary for degraded communities). Requires `--features community` (the value parses without it but the run fails with an actionable error, mirroring `--backend llms` without `llms-backend`) |
| `communities-hierarchy` | `{detector: "hierarchical-leiden", num_levels, levels: [{level, quality, num_communities, communities}]}` — hierarchical Leiden levels ordered coarse → fine (`level` 0 matches a flat Leiden run's grouping), each level carrying its modularity; fixed-seed detection + sorted member ids make the output deterministic. With `--community-summaries`, **every level's** communities render as `{members, name, summary}` objects (`null` fields for degraded communities). Requires `--features community-leiden` (the value parses without it but the run fails with an actionable error naming the feature) |
| `mermaid` | `graph LR`; entity ids/labels cleaned of `[`/`]`; one `-->| label |` line per triple; fixed styling trailer |
| `stats` | `{num_entities, num_triples, entity_types:{}, predicate_types:{}, num_segments_processed}` |

[T] (`graph::to_node_link_uses_source_target_referencing_node_ids`,
`protocol::knowledge_graph_converts_to_portable_kg_protocol`)

## 9.5 Pre-chunked input format

Accepted shapes (ABNF for the JSON envelope is omitted; the shapes are):

- a JSON array of chunk objects,
- a `{"chunks":[...]}` truncation wrapper,
- JSONL (one chunk object per line; a trailing `{"truncated":...}` metadata
  line is skipped).

Each chunk **MUST** have a `text` field. Character offsets are read from
`range.char_span.start`/`end`; if absent, synthesized cumulatively (monotonic).
Line ranges come from `range.line.start`/`end` (1-based). Source file from
top-level `source_file` (`"<stdin>"` treated as unknown). A chunk without a
`text` field is an error; empty input and `[]` are errors. A non-object
`metadata` is an error.

One run **MUST** cover a single source document: conflicting non-empty
`source_file` values are rejected with an actionable error naming the
offending chunk index, both files, and the remedy (split the input by
`source_file`, run once per document).

The optional `title` (string) and `metadata` (JSON object — e.g.
kg-multimodal's `mm_*` provenance keys) **MUST NOT** be dropped at the input
boundary: the chunk-aware engines (Simple/Agentic) stamp them onto every
record extracted from that chunk as the metadata keys `chunk_title` /
`chunk_metadata`, which the protocol projection passes through to
entity/relation properties. Single-shot engines (SchemaJson/ToolCall) join the
chunk texts and therefore retain document-level provenance only — the
per-chunk payload is not preserved there.
[T] (`chunking::prechunked_parses_jsonl_with_metadata`,
`chunking::prechunked_rejects_chunk_without_text_and_empty_input`,
`chunking::prechunked_preserves_title_and_metadata`,
`chunking::prechunked_rejects_non_object_metadata`,
`chunking::prechunked_rejects_conflicting_source_files`,
`simple::tests::prechunked_title_and_metadata_reach_protocol_properties`)

## 9.6 MCP server (`kg-extract-mcp`)

A stdio MCP server wrapping a `KgStore`. It does **not** call a model; the
client drives the mutations. Entity identity is the shared `md5(name)` scheme,
so MCP-produced files are interchangeable with engine output.

```
   MCP Client                       kg-extract-mcp (KgStore)
     |                                  |
     |--- tools/call add_entity ------->|  load <path>.json
     |                                  |  validate (schema policy, citation)
     |                                  |  merge delta, save
     |<---- result {ok,path,stats} -----|
     |                                  |
     |--- tools/call add_relation ------>|  require both endpoints present
     |                                  |  dedup by (s,p,o); union citations
     |<---- result ---------------------|
```

### Path resolution contract

`path` maps to `<output>/<path>.json`. Absolute paths and any `..` component
are **rejected**; the result can never escape `<output>`. A path resolving to
the output directory itself is rejected. [T] (mcp_test.rs path-safety tests)

### Source-citation validation

`add_entity`/`add_relation` accept an optional `(source_file, start_line,
end_line)` group. When provided:

- `source_file` **MUST** be a relative path under `source_root` (absolute and
  `..` rejected); it **MUST** exist as a regular file there;
- `start_line`/`end_line` are 1-based, `start_line ≤ end_line`, and the range
  **MUST NOT** exceed the file's line count.

A violation returns a tool error so the client can correct the path/lines;
valid citations are written to `metadata.citations`. Repeated calls for the
same entity/relation merge citations rather than duplicating the record.

### Schema policy (MCP)

`Open` accepts any caller-supplied type names. `Fixed` accepts only the seed
schema types. `Evolving` accepts seed types plus any type the client has
explicitly proposed via `propose_schema_type` for that graph path (persisted
under `new_schema_types`, then allowed by later mutations). `propose_schema_type`
is rejected unless the policy mode is `Evolving`.

## 9.7 Provider protocol (`kg.provider/v1`)

Three subcommands expose kg-extract to an external **capability hub**
(kg-acme). They are the machine-facing contract; the human `--describe` flag
stays as-is and points at them.

```abnf
describe  = "describe" "--json"          ; exactly one manifest JSON on stdout
available = "available" "--json"         ; exit code ALWAYS 0; misses in JSON
invoke    = "invoke" capability-id "--request" ("-" / file) ["--artifacts-dir" dir]
```

- **`describe --json`** — one `kg.provider/v1` document:
  `{protocol, protocol_versions: [1], provider: {id: "kg-extract", version:
  CARGO_PKG_VERSION, description}, capabilities: [...]}`. Each capability
  carries `capability_id` (stable, hub match key), `title`, `description`,
  `side_effects`, `input_schema` (every property has a self-explanatory
  `description`; enums are `oneOf` `const`s with per-value descriptions),
  `output: {mode: "artifact"|"result-json", kind}`, and `cli_spec`.
- **`cli_spec`** — argv template following the hub's provider-v1 schema
  (`subcommand`/`always`/`positionals`/`flags[]` with
  `name/flag/kind/optional/default/repeatable/join/stdout/order/negated`;
  flag `kind` ∈ `string|number|boolean|array`; emission order
  `always ++ subcommand ++ positionals ++ flags(by order, tiebreak flag)`).
  For `extract.entities_relations` it renders the flat-flag
  argv (`kg-extract -o kg-protocol …`; the `text` field rides stdin, no flag).
  The graph-in capabilities have no flat-flag equivalent (extraction flags
  would imply an LLM call), so their cli_spec renders the `invoke` form
  itself. [T] (`tests::extract_cli_spec_renders_argv_equivalent_to_direct_cli`,
  `tests::graph_in_cli_specs_render_invoke_argv_that_parses`)
- **`available --json`** — `{available, ready: [{name, kind}], missing:
  [{name, kind}]}` (`cache_dir` omitted: there is no local weight cache, and
  the hub schema types it as a string); `available` is true only when nothing
  is missing. Probes are read-only (env vars via the backend's own provider-env
  resolution, PATH, compiled features); no network calls.
- **`invoke <capability_id> --request (-|file)`** — stdin/file JSON request →
  exactly one `kg.execution/v1` envelope on stdout (logs/human output go to
  stderr). The request is accepted in both the hub's wrapped form
  `{"capability_id", "input"}` (kg-acme spec 01 §3; a mismatched
  `capability_id` is `invalid_request`) and the bare input-object form for
  direct CLI use. Envelope: `{protocol, capability_id, provider, status: "ok"|"error", result,
  artifacts: [{path, kind, checksum: "sha256:<hex>"}], diagnostics:
  [{severity, message}], error: {code, message}?}`. Artifacts are written to
  `--artifacts-dir` (default: a fresh temp dir). `status: "error"` ⇒ exit
  code non-zero.

Capabilities (ids frozen; graph-in capabilities import `kg.protocol.v1` via
`KnowledgeGraph::from_kg_document`):

| capability_id | in → out | side_effects | feature |
|---|---|---|---|
| `extract.entities_relations` | text/file → `kg-document` artifact | `network`, `data_egress` | — |
| `detect.communities` | kg-document → `communities` result | — | `community` |
| `detect.communities_hierarchy` | kg-document → `communities` (hierarchy) result | — | `community-leiden` |
| `summarize.communities` | kg-document → summarized communities result | `network`, `data_egress` | `community` (`community-leiden` when `hierarchy`) |
| `resolve.coref` | kg-document → `kg-document` artifact | — | — |
| `resolve.canonical_direction` | kg-document → `kg-document` artifact | — | — |

`resolve.coref` / `resolve.canonical_direction` run the same passes as the
extraction-time `coref` / `canonical_direction` options, exposed
graph-in/graph-out so a hub can chain them after any graph producer. A
capability whose feature is not compiled in still appears in `describe`
(stable surface) but its invoke fails with `backend_unavailable`, mirroring
the `-o communities` / `--backend llms` convention.

**Error model** (`error.code`): `invalid_request` (malformed JSON, contract
violations — unknown/conflicting/missing fields, bad enums, unreadable
files), `unknown_capability`, `backend_unavailable` (missing compiled feature
or unconstructable backend), `extraction_failed` (engine/summary run
failure).

**Import rule** (`from_kg_document`, protocol.rs): type tokens re-resolve
through the kg-vocab `resolve` (exact → alias → OTHER fallback, original
token kept as `raw_type`); `normalized_*`/`predicate_metadata` properties are
re-derived, not round-tripped; ranged evidence becomes internal citations
(re-promoted on export); relations with dangling endpoints are dropped and
counted; range-less evidence is dropped and counted. Drops are never silent —
they surface as invoke diagnostics. [T]
(`protocol::tests::from_kg_document_*`)

## 12. Extension points

- **Backend** — implement `LlmBackend` (+ optionally `complete_with_tools`,
  `open_session`) to plug a new completion source. The contract that MUST be
  preserved: a single `complete` turn returns assistant text; tool calls, when
  advertised, follow the OpenAI tool-call message shape.
- **Chunker** — `ChunkStrategy` selects the segmenter; `Char` reproduces the
  Python character window 1:1. A new chunker MUST emit segments carrying char
  offsets so citations remain code-computed.
- **Template / preset** — a YAML template (shipped preset or user file) drives
  the schema-json prompt from its guideline + output fields; the output JSON
  contract is unchanged, so a template steers *what* is extracted, not the
  wire format.
- **Output format** — `print_response` dispatches one terminal format; a new
  format MUST consume the same `KnowledgeGraph` and not alter the graph.
