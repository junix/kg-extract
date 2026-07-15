# 06 — Feature Matrix

Planning and coverage-tracking document. **Not** a normative contract: rows
are capabilities, the `Spec` column points to the chapter that holds the
contract. Rows MUST NOT be read as RFC 2119 promises.

## Legend

| Field | Values |
|-------|--------|
| **Maturity** | `stable` / `experimental` / `internal` / `deprecated` / `removed` |
| **Spec** | `done` (full behavior + DoD) / `partial` (mentioned, gaps) / `missing` (no chapter) / `n/a` (out of scope) |
| **Surfaces** | `lib` (Rust API) / `cli` / `mcp` (stdio server) / `cfg` (config) |

## Feature inventory

### Extraction engines

| ID | Feature | Surfaces | Maturity | Flag | Tests | Spec |
|----|---------|----------|----------|------|-------|------|
| E-01 | Simple extractor (delimiter prompt + gleaning) | lib, cli | stable | — | simple_test.rs | done → 03 |
| E-02 | SchemaJson extractor (JSON prompt + modes) | lib, cli | stable | — | schema_json tests | done → 03 |
| E-03 | ToolCall extractor (tool/function calling) | lib, cli | stable | — | toolcall tests | done → 03 |
| E-04 | Agentic extractor (single sandboxed session) | cli | experimental | — | agentic_test.rs | partial → 03 |

### Schema modes

| ID | Feature | Surfaces | Maturity | Flag | Tests | Spec |
|----|---------|----------|----------|------|-------|------|
| S-01 | Open mode (default, free) | lib, cli, mcp | stable | — | schema_json/toolcall tests | done → 02,03 |
| S-02 | Fixed mode (closed-world hard-drop) | lib, cli, mcp | stable | `--schema-mode fixed` | fixed_* tests | done → 02,03 |
| S-03 | Evolving mode (record new types) | lib, cli, mcp | stable | `--schema-mode evolving` | evolving_* tests | done → 02,03 |
| S-04 | Empty-schema degenerate-cell rejection | lib, cli | stable | — | *_without_schema_errors | done → 02,03 |
| S-05 | Spec/execution split (`with_spec`, both engines) | lib | stable | — | one_spec_runs_through_both_engines | done → 02,04 |

### Graph construction & identity

| ID | Feature | Surfaces | Maturity | Flag | Tests | Spec |
|----|---------|----------|----------|------|-------|------|
| G-01 | Deterministic md5 entity id | lib, mcp | stable | — | entity_id_is_deterministic_md5 | done → 02,03 |
| G-02 | Dangling-endpoint drop (engines) | lib | stable | — | dangling_relation_is_dropped | done → 03 |
| G-03 | Type normalization audit (Exact/Aliased/Fallback) | lib, cli | stable | — | type_normalization_* | done → 02 |
| G-04 | Passive `*_BY` auto-reverse (Simple) | lib | stable | — | passive_by_relation_* | done → 03 |
| G-05 | GraphBuilder name-resolution core | lib, mcp | internal | — | (via engine tests) | done → 03 |

### Merge / coreference

| ID | Feature | Surfaces | Maturity | Flag | Tests | Spec |
|----|---------|----------|----------|------|-------|------|
| M-01 | Four merge strategies | lib, cli | stable | `--merge-strategy` | merger tests | done → 02,03 |
| M-02 | Fuzzy cross-chunk coreference | lib, cli | stable | `--coref` | coref_fuzzy_* | done → 02,03 |
| M-03 | LLM-synthesised description merge | lib, cli | stable | `--merge-strategy llm` | llm_strategy_synthesizes_* | done → 02,03 |
| M-04 | Citation union on merge | lib | stable | — | citations_stamp_*_union_on_merge | done → 02,03 |

### Provenance

| ID | Feature | Surfaces | Maturity | Flag | Tests | Spec |
|----|---------|----------|----------|------|-------|------|
| P-01 | Legacy line / rich SourceRange citations | lib | stable | — | citation tests | done → 02,03 |
| P-02 | Chunk-aware pre-chunked page/bbox provenance | lib, cli | stable | `-F chunks` | prechunked_multimodal_range_* | done → 02,03 |
| P-03 | Multi-citation accumulation | lib | stable | — | citations_stamp_* | done → 02 |

### Chunking & input

| ID | Feature | Surfaces | Maturity | Flag | Tests | Spec |
|----|---------|----------|----------|------|-------|------|
| C-01 | Char / Recursive / Token chunking | lib, cli | stable | `-c/--chunker` | char_segment_size_splits, recursive_honors_segment_size | done → 03 |
| C-02 | Pre-chunked input parsing (one source, full SourceRange) | cli | stable | `-F chunks` | prechunked_* | done → 03,04 |

### Output formats

| ID | Feature | Surfaces | Maturity | Flag | Tests | Spec |
|----|---------|----------|----------|------|-------|------|
| O-01 | JSON | cli | stable | `-o json` | (default) | done → 04 |
| O-02 | JSONL | cli | stable | `-o jsonl` | print_response wiring | partial → 04 |
| O-03 | kg-protocol (citations → Evidence, no property duplicate) | cli | stable | `-o kg-protocol` | protocol::knowledge_graph_*, prechunked_multimodal_range_* | done → 02,04 |
| O-04 | node-link | cli | stable | `-o node-link` | to_node_link_* | done → 04 |
| O-05 | LadybugDB import | cli | stable | `-o ladybug-import` | ladybug e2e scripts | partial → 04 |
| O-06 | Mermaid | cli | stable | `-o mermaid` | (via mermaid emit) | done → 04 |
| O-07 | stats | cli | stable | `-o stats` | (get_stats) | done → 04 |

### MCP server

| ID | Feature | Surfaces | Maturity | Flag | Tests | Spec |
|----|---------|----------|----------|------|-------|------|
| X-01 | KgStore load-modify-save | mcp | stable | — | mcp_test.rs | done → 03,04 |
| X-02 | Path sandboxing (no escape) | mcp | stable | — | mcp_test.rs | partial → 04 |
| X-03 | Source-citation validation | mcp | stable | `--source-root` | mcp_test.rs | partial → 04 |
| X-04 | Schema policy (open/fixed/evolving) | mcp | stable | — | mcp_test.rs | partial → 04 |
| X-05 | query_graph views | mcp | stable | — | mcp_test.rs | partial → 04 |

### Templates / presets

| ID | Feature | Surfaces | Maturity | Flag | Tests | Spec |
|----|---------|----------|----------|------|-------|------|
| T-01 | Bundled preset gallery (embedded) | cli | stable | `--list-presets` | template/tests.rs | partial → 04 |
| T-02 | Preset / preset-file routing | cli | stable | `--preset`, `--preset-file` | template_extracts_under_fixed_* | done → 04 |

### Backends

| ID | Feature | Surfaces | Maturity | Flag | Tests | Spec |
|----|---------|----------|----------|------|-------|------|
| B-01 | LlmsBackend (in-process) | lib, cli | stable | `--features llms-backend` | (integration) | partial → 04 |
| B-02 | SdkAgentBackend (stream-json) | lib, cli | stable | `-b agent` | (integration) | partial → 04 |
| B-03 | PiAgentBackend | lib, cli | stable | `--agent pi-agent` | (integration) | partial → 04 |
| B-04 | MockBackend (deterministic) | lib, cli | internal | `-b mock` | (used pervasively) | done → 04 |

## Summary statistics

| Spec status | Count |
|-------------|-------|
| done | 26 |
| partial | 11 |
| missing | 0 |
| n/a | 0 |

## Spec writing priority (informative)

The `partial` rows concentrate in three clusters that a follow-up `EXTENDED`
pass could close:

1. **MCP server (X-02..X-05)** — read `mcp_test.rs` in full, promote the
   `[U]` DoD items in chapter 05 cluster K to `[T]`, and lift X rows to done.
2. **Output formats O-02, O-05** — pin the JSONL record shape and the
   LadybugDB table/metadata shape with ABNF / examples.
3. **Backends B-01..B-03** — these are integration-shaped; document only the
   externally observable contract (one-shot / tool-call / session), not
   subprocess internals.

## Maintenance

- A new externally observable capability → add a row with `Spec = missing`
  until a normative chapter covers it.
- A retired capability → mark `removed` and update the normative chapter.
- After any normative chapter change, set the touched rows' `Spec` column to
  `partial` or `done` to avoid coverage drift.
