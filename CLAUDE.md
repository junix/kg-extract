# kg-extract Agent Guide

Multi-strategy **knowledge-graph extraction** in Rust — a behavioural port of the
Python `graph.kg_extractor` module. Turns unstructured text into a typed
`KnowledgeGraph` (typed entities + predicate-typed triples), using one of four
extraction engines behind a common `Extractor` trait, with pluggable LLM
backends. Ships as a library crate, a CLI (`kg-extract`), and an optional stdio
MCP server (`kg-extract-mcp`).

## Build & test

The repo uses a `justfile`; feature flags gate the heavy backends:

```bash
just build          # cargo build --release --features "llms-backend mcp community community-leiden"
just test           # cargo test --features mcp
just lint           # cargo clippy --all-targets --features "llms-backend mcp community community-leiden"
just install        # copies kg-extract + kg-extract-mcp to ~/sync/<os>-<arch>-bin/ (override with SYNC_BIN_DIR)
```

Without `--features llms-backend` only the agent-CLI and Mock backends build.
`--features mcp` is required to compile the `kg-extract-mcp` binary. End-to-end
evaluation lives in `scripts/ladybug-e2e-{smoke,eval}.sh` (`just ladybug-smoke`,
`just ladybug-eval*`).

## The two orthogonal axes (read this before touching an engine)

Engines vary along **two independent dimensions** — keep them separate in your
head and in code:

1. **Mechanism** (how the model is driven):
   - *Prompt → parse* — `SimpleExtractor` (delimiter format + multi-gleaning for
     recall) and `SchemaJsonExtractor` (schema-guided JSON). Output is parsed.
   - *Tool call → structured* — `ToolCallExtractor`: the model calls typed
     `add_entity` / `add_relation` tools; structured by construction, **no parsing**.
     The MCP server (`src/mcp.rs`) exposes the *same* graph-building core to an
     *external* agent.
2. **Schema mode** (`SchemaMode`: `Open` / `Fixed` / `Evolving`) — how a seed
   schema constrains types. Orthogonal to mechanism; first-class on SchemaJson,
   ToolCall, and Agentic. `Fixed`/`Evolving` **require a non-empty seed schema**
   (constraining to / evolving from an empty schema is rejected, not silently
   degraded). `Fixed` is enforced differently per engine (ToolCall:
   JSON-Schema `enum`; SchemaJson/Agentic: parse-then-drop, Agentic feeds the
   drop back into the next turn).

## Architecture / data flow

```
text ──chunking (chonkie)──► LlmBackend.complete() ──► parse ──► merge/dedup ──► KnowledgeGraph
                                                              │
              Recursive (default) / Char (Python-parity) / Token        JSON · node-link · Mermaid · stats
```

Key modules under `src/`:

- `extractor/` — the four engines (`simple/`, `schema_json.rs`, `toolcall.rs`,
  `agentic/`) + the `Extractor` trait. `AgenticExtractor` runs the whole document
  through one multi-turn SDK session (slices fed as turns; can `grep` the doc).
- `backend/` — the `LlmBackend` trait and impls: `LlmsBackend` (in-process
  `llms` crate, feature-gated), `SdkAgentBackend` (drives `minimaxcc`/`glmcc`/
  `mimocc` via `claude-agent-sdk-rs` stream-json), `PiAgentBackend` (pi-rs
  `pi-agent`, different CLI contract), `MockBackend` (tests). Native multi-turn
  sessions (`open_session`) live on SdkAgent; others fall back to
  `ReplaySession` which replays history each turn.
- `graph_build.rs` — **shared graph-construction core**: the deterministic
  `entity_<md5(name)[..8]>` id scheme, name-based relationship resolution,
  dangling-endpoint dropping. Shared by every extractor *and* the MCP store so
  their outputs are interchangeable. Type/predicate *parsing* stays at the call
  site (engines differ in fallback semantics).
- `merger.rs` — dedup/merge of entities (lowercased label) and triples
  (`(subj_id, predicate, obj_id)`). Coref: exact → normalised (strips corp
  suffixes/articles) → two similarity channels under `CorefMode::Fuzzy`, both
  gated on type compatibility: edit distance (`FUZZY_MIN_LEN=6` / threshold
  `0.85`) and token-set (multi-token subset, or Jaccard ≥ 0.6; no length
  gate). `MergeStrategy`: `KeepExisting` (default) / `KeepIncoming` /
  `FieldUnion` / `Llm`.
- `chunking.rs` — thin layer over the sibling `chonkie` crate. Public `Segment`
  carries `range: Option<core_types_rs::SourceRange>` so pre-chunked char, line,
  page, and bbox coordinates survive the extraction boundary, plus the optional
  pre-chunked `title`/`metadata` payload (kg-multimodal's `mm_*` keys).
- `citation.rs` / `protocol.rs` — provenance. Legacy line-only entries remain
  `{doc, lines:[start,end]}`; entries with page/bbox use
  `{doc, range:SourceRange}`. `to_kg_document` promotes a fully recognized
  citation array to first-class `Evidence` and removes that internal key from
  protocol properties; malformed/foreign user values stay untouched.
  Plain-text line ranges are computed from chunker char offsets by our code;
  the model is never asked to count lines. Pre-chunked `title`/`metadata`
  payloads ride the same chunk→record stamping channel: chunk-aware engines
  attach them as `chunk_title` / `chunk_metadata` metadata keys, which
  `to_kg_document` passes through to protocol properties.
- `template/` + `presets/` — extraction **templates/presets**: richer than a flat
  `Schema` (output structure + multilingual guideline + identifier conventions).
  `presets/**.yaml` are embedded into the binary via `include_dir`; load by
  `{domain}/{name}` key (e.g. `general/concept_graph`).
- `types/` — `KnowledgeGraph`, `Entity`, `Triple`, `Predicate`, `Schema`,
  `ExtractionConfig`, `ExtractionSpec` (the declarative spec holds `SchemaMode`
  to avoid a types→extractor cycle). **`EntityType`/`PredicateType`/`TypeMatch`
  are re-exported from the `kg-vocab` crate** (single source of truth for the
  122/108 vocabulary; ADR-987 Step 5) — `types/entity.rs`/`predicate.rs` keep
  only the rich `Entity`/`Predicate` structs.
- `community.rs` *(feature `community`)* — `KnowledgeGraph → kg_community::Graph`
  adapter + `detect_communities*`. Every triple becomes one weight-1.0 edge and
  parallel edges are summed by the detectors, so triple multiplicity acts as
  edge weight (do NOT reintroduce `Graph::from_edges` dedup here). CLI surface:
  `-o communities` (label propagation → `community id → member ids` JSON with a
  `quality` field, `null` for label propagation) and `-o communities-hierarchy`
  (hierarchical Leiden, coarse→fine levels with per-level modularity; feature
  `community-leiden` forwards to `kg-community/leiden`).
  `--community-summaries` adds GraphRAG-style reports: `summarize_partition`
  calls the backend once per community (per level in hierarchy mode) for a
  `{name, summary}` JSON reply, merged into the community object as
  `{members, name, summary}`; prompt bounded by `SUMMARY_MAX_*` constants,
  calls run with bounded concurrency (`--max-concurrency`) and results are
  keyed by community label so the output is deterministic regardless of
  completion order (byte-identical to a sequential run), failed/unparseable
  calls degrade to null fields + stderr warning instead of failing the run.
  Wires the extraction output into the (previously orphan) `kg-community`
  detectors (ADR-987 §D#2).
- `ladybug_export.rs` — export to the `graphdb-ladybug` graph store (e2e flows).

## CLI essentials

`kg-extract` settings precedence: CLI flag > `--config` (or `~/.kg-extract/config.json`)
> built-in default. See `config.example.json`. Key flags: `-e/--engine`
(`simple`|`schema-json`|`toolcall`|`agentic`), `-b/--backend`
(`llms`|`agent`|`mock`), `--agent` (`minimaxcc`|`glmcc`|`mimocc`), `--chunker`
(`recursive`|`char`|`token`), `--schema-mode`, `--schema`, `-f/--input-format`
(`text`|`chunks` — chunk-aware Simple/Agentic consume supplied ranges as-is:
page/bbox emits the complete rich range, line-only stays legacy, and the
chunk's `title`/`metadata` payload is stamped as `chunk_title`/`chunk_metadata`;
SchemaJson/ToolCall join texts and retain only document-level provenance),
`-o/--output`
(`json`|`jsonl`|`kg-protocol`|`node-link`|`ladybug-import`|`communities`|`communities-hierarchy`|`mermaid`|`stats`;
`communities` requires `--features community`, `communities-hierarchy` requires
`--features community-leiden`), `--community-summaries` (per-community LLM
`name`/`summary` merged into the two community output formats).

## Conventions & gotchas

- **Parity is intentional.** Several Python quirks are preserved (documented in
  code): `SimpleExtractor`'s relationship tuple field-shift (the relationship-type
  token drives predicate inference) and exact-match-before-alias entity typing.
  Don't "fix" these without checking the Python original.
- **`graph_build.rs` is the shared seam.** New engines or the MCP store must use
  `entity_id` / `GraphBuilder` so outputs stay interchangeable; keep
  type/predicate *parsing* at the call site.
- **`src/bin/*_test.rs` are pulled in via `#[path]`, not real binaries** — that's
  why `autobins = false` in `Cargo.toml`. Don't "fix" that.
- **`SchemaMode` lives in `types::spec`**, re-exported from `extractor` — don't
  move it into `extractor` or you create a types→extractor dependency cycle.
- **Agentic is a consolidation/coref strategy, not a recall one** — its
  extraction-time dedup is *not* reproducible by a post-hoc merge pass (see the
  README trade-off table). Keep slices strictly sequential.
- **Citations are code-owned, not model-emitted** — derive plain-text lines
  from offsets, retain pre-chunked ranges on `Segment`, and apply the rich vs
  legacy encoding rule above; never ask a model to invent coordinates.
- **`Segment.lines` became `Segment.range`.** This is a Rust source-breaking
  public-struct change; update struct literals and field access. The accepted
  pre-chunked JSON remains wire-compatible because `range` and its optional
  coordinates are protocol fields. `Segment` later also gained `title` /
  `metadata` (the pre-chunked payload) — same story: struct literals need the
  new fields, the wire format only grew optional keys.
- **`Citation` now stores `range`, not public `start_line` / `end_line`, and is
  `PartialEq` but not `Eq`** because bbox coordinates are floating-point.
  `Citation::new(doc, start, end)` remains compatible for valid line ranges.
- **Protocol-facing Git dependencies use exact `rev` pins and a tracked
  `Cargo.lock`.** Verify shared changes with `cargo test --lib --locked`. A
  local path patch may be supplied transiently through Cargo `--config`, but
  path overrides MUST NOT be committed to `Cargo.toml` or the lockfile.
- Design specs live in `spec/` (`00-glossary.md` …
  `07-protocol-contract-lock.md` + `CHANGELOG.md`) — read these before
  non-trivial design changes.
- Reference: every project under `~/projects` must have a `README.md` (this one
  is extensive; keep it in sync with engine behaviour).
