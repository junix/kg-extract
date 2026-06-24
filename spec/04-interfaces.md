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
flag (`--coref`) wins over the config. `--list-presets` prints the gallery and
exits.

```abnf
engine      = "simple" / "schema-json" / "toolcall" / "agentic"
backend     = "llms" / "agent" / "mock"
chunker     = "char" / "recursive" / "token"
schema-mode = "open" / "fixed" / "evolving"
merge-strat = "keep-existing" / "keep-incoming" / "field-union" / "llm"
input-fmt   = "text" / "chunks"
out-fmt     = "json" / "jsonl" / "kg-protocol" / "node-link"
            / "ladybug-import" / "mermaid" / "stats"
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
| `--merge-strategy` | how label-duplicates fold |

## 9.4 Output formats

| Format | Shape |
|--------|-------|
| `json` *(default)* | `{entities: {id: Entity.to_dict}, triples: [Triple.to_dict], metadata}` |
| `jsonl` | one line per record: `{kind:"entity"\|"triple", data: …}` |
| `kg-protocol` | portable `KgDocument` (`schema_version`, `entities`, `relations`, `evidence`); relations reference entity ids; citations → first-class evidence ranges |
| `node-link` | `{directed:true, nodes:[{id,label,…}], links:[{source,target,type,…}]}` — `source`/`target` reference node `id`s; no RDF `subject`/`object` keys leak |
| `ladybug-import` | generic `KgEntity` node table + one relationship table per predicate; metadata stored as JSON strings |
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
`text` field is an error; empty input and `[]` are errors.
[T] (`chunking::prechunked_parses_jsonl_with_metadata`,
`chunking::prechunked_rejects_chunk_without_text_and_empty_input`)

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
