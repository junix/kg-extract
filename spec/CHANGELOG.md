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
