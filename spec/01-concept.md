# 01 — Concept

## Notational conventions

This specification uses the following notations. A notation is used only where
the corresponding contract appears.

1. **RFC 2119 / BCP 14 keywords.** Normative statements use the uppercase
   keywords **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY**.
   The lowercase adverbs "usually", "generally", "should", "will", "typically"
   do not appear in normative statements; any such wording is `(informative)`.

2. **ABNF (RFC 5234 + RFC 7405).** Input syntaxes (the Simple engine
   delimiter record grammar, the CLI schema-JSON relationship shapes, the
   pre-chunked JSON input) are given as ABNF. Non-terminals are either defined
   here or drawn from RFC 5234 §B.1 (`ALPHA`, `DIGIT`, `SP`, `CRLF`, `DQUOTE`).

3. **ASCII sequence diagrams.** Multi-party flows (the Simple per-chunk
   session, the ToolCall multi-round loop, the KgStore load-modify-save cycle)
   use ASCII sequence diagrams.

4. **State transition tables.** State-bearing behaviour (the ToolCall
   round loop, the KgStore per-path graph lifecycle, schema-mode gating) is
   given as `(state, event) → (state', action, output)` tables.

5. **Paper-style pseudo-code (`Algorithm <n>`).** Every externally observable
   operation carries one Algorithm block with a `Require:` / `Ensure:` pair,
   numbered lines, `←` assignment, and `▷ …` comments.

This specification does NOT introduce TLA+, Z notation, Petri nets, or LTL.

## 1. Scope

The object of this specification is **kg-extract**: a system that turns
unstructured text into a **knowledge graph** of typed entities and
predicate-typed triples, using one of four extraction engines behind a common
interface, plus a companion **MCP** store that accumulates graphs from an
external agent without invoking a model.

Boundary:

- IN scope: the four engines (Simple, SchemaJson, ToolCall, Agentic), the
  schema modes (Open / Fixed / Evolving), the merge / coreference / citation
  contracts, the CLI, the output formats, and the MCP KgStore.
- OUT of scope: the specific completions returned by any particular model
  provider, the agent-CLI subprocess wire protocol internals, and the
  rendering quality of the LadybugDB import.

## 2. Problem statement

A knowledge graph is wanted from free text, but text exceeds a single model
context, models emit facts in inconsistent shapes, and the same entity
surfaces under different names across chunks. kg-extract exists to:

- segment text into processable units,
- drive a model through one of four mechanisms to emit entity/relation facts,
- normalize those facts into a typed, deduplicated graph,
- attach provenance that cannot be hallucinated, and
- expose the result as several interchangeable serializations.

## 3. Goals

The current code does the following:

- Four engines share one `Extractor` interface producing one `KnowledgeGraph`.
- Three orthogonal schema modes constrain (or do not constrain) extraction.
- Entities carry a deterministic id so graphs are interchangeable across
  engines and the MCP store.
- Provenance (document + 1-based inclusive line range) is computed by the
  code from chunk offsets, never by the model.
- Duplicate recognition (exact or fuzzy coreference) and four merge
  strategies fold per-chunk graphs into one.
- The graph is emitted as JSON, node-link, kg-protocol, LadybugDB import,
  Mermaid, JSONL, or stats.
- An MCP server accumulates the same graph shape from external tool calls.

## 4. Non-Goals

The current code explicitly does NOT:

- guarantee recall or precision of the extracted facts (that depends on the
  model),
- support custom extractors beyond the `Extractor` trait (no plugin
  registry),
- persist extraction state across runs (graphs are returned, not stored,
  except via the MCP store),
- provide an explicit timeout / retry policy around model calls (a model
  error surfaces or, for some engines, yields an empty graph),
- bound the conversation context of the Agentic engine beyond slicing.

## 6. Design principles

The code enforces these invariants:

- **Identity is deterministic.** `entity_id(name) = "entity_" ++ md5(name)[0..8]`,
  keyed on the raw name bytes (case-sensitive). Every engine and the MCP
  store share this scheme.
- **Open-schema is lossless.** When no schema constrains a type token, the
  raw model token is preserved (`raw_type`); the enum normalization is a
  separate, auditable field.
- **Provenance is code-computed.** Line ranges derive from chunk char
  offsets; the model is never asked to count lines.
- **Dangling endpoints are dropped, never stubbed.** A relation whose
  endpoint is unknown is discarded (engines) or rejected (MCP store).
- **Passive `*_BY` predicates point from the thing acted on to the doer.**
  The Simple engine additionally auto-reverses an actor→non-actor `*_BY`
  misdirection.
- **The degenerate schema cell is unrepresentable.** Fixed / Evolving on an
  empty schema is rejected, not silently degraded.

## 7. Dependencies / Companion interfaces

kg-extract depends on the following companion *capabilities* (named by
interface, not product):

- **A text chunker** able to segment text into spans carrying char offsets
  and (optionally) line ranges.
- **A completion backend** exposing: one-shot chat, optional tool/function
  calling, and an optional stateful multi-turn session. A replay-session
  fallback covers backends without a native session.
- **A portable KG protocol type** (`core-types-rs` `KgDocument`) for the
  `kg-protocol` output, with first-class evidence ranges.
- **An agent-SDK client** (for the Agentic engine) able to run in a
  read-only tool sandbox with a configured working directory and one
  long-lived multi-turn session.
- **The MCP server SDK** (for `kg-extract-mcp`) dispatching stdio tool calls.
