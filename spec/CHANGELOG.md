# CHANGELOG

Append-only session ledger for the kg-extract specification. Each non-STABLE
mode appends exactly one entry.

## 2026-06-24T00:19 — CREATED

Full normative spec authored for kg-extract (no prior `spec/`).

- Files: 00, 01, 02, 03, 04, 05, 06
- Code basis: a0b4de0
- Seed scope: all four engines (Simple/SchemaJson/ToolCall/Agentic), the three
  schema modes, merge/coref/citation contracts, CLI + 7 output formats, and
  the MCP KgStore. Evidence drawn from tests + code + README.
- Feature matrix: 37 rows; 26 done / 11 partial / 0 missing. Partial clusters:
  MCP server (X-02..X-05), output formats O-02/O-05, backends B-01..B-03 —
  recommended targets for a follow-up EXTENDED pass.
- Notable: `[U]` DoD items in chapter 05 cluster K (MCP KgStore) await an
  explicit Test↔DoD mapping over `mcp_test.rs`; the contract statements are
  normative pending that tag promotion. Type-vocabulary size, alias tables,
  corporate-suffix/article lists, Agentic slice-count, and exact prompt
  wording recorded as implementation-defined, not contractual constants.

## 2026-07-15T09:21 — UPDATED

Aligned the specification with the cross-package provenance contract lock.

- Files: 00, 01, 02, 03, 04, 05, 06, 07, CHANGELOG.
- Citation wire contract: legacy line-only entries remain `{doc,lines}`;
  citations carrying page/bbox use `{doc,range:SourceRange}` and preserve the
  complete supplied range.
- Protocol projection: a fully recognized array in either citation shape
  becomes first-class `Evidence`; its internal key is removed to avoid a
  duplicate representation. Foreign, malformed, or mixed `citations` values
  stay in protocol properties rather than being partially promoted or lost.
- Engine boundary: Simple/Agentic retain per-chunk provenance; single-shot
  SchemaJson/ToolCall join chunks and provide document-level provenance only.
  Conflicting non-empty pre-chunked `source_file` values are rejected.
- Rust API compatibility: public `Segment.lines` became `Segment.range`, and
  `Citation.{start_line,end_line}` became `Citation.range`. Both break direct
  struct literals/field access; `Citation` is now `PartialEq` but not `Eq`
  because bbox coordinates are floating-point. `Citation::new(doc,start,end)`
  remains compatible for valid line ranges. Pre-chunked JSON and legacy
  line-citation JSON remain wire-compatible through optional protocol `range`
  fields and the unchanged `{doc,lines}` form.
- Build reproducibility convention: protocol-facing Git dependencies require
  exact revisions, `Cargo.lock` is tracked, and local path patches remain
  transient rather than committed manifest/lock state.
