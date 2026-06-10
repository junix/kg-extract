# kg-extract

Multi-strategy **knowledge-graph extraction** in Rust — a faithful port of the
Python `graph.kg_extractor` module. Turns unstructured text into a
`KnowledgeGraph` of typed entities and predicate-typed triples, using one of
four extraction strategies behind a common trait.

## Strategies

| Extractor | Approach | Default model |
|-----------|----------|---------------|
| `SimpleExtractor` | General LLM chat with GraphRAG-style **delimiter prompting** + **multi-gleaning** (iteratively asks "what did you miss?" for high recall) | `qwen-max` |
| `SchemaJsonExtractor` | **Schema-driven** JSON extraction with three **schema modes**: open / fixed / evolving | `qwen-max` |
| `ToolCallExtractor` | **Tool / function calling** — typed `add_entity` / `add_relation` / … tools; structured by construction, **no output parsing**; same open / fixed / evolving **schema modes** | `qwen-max` |

All three implement the `Extractor` trait:

```rust
#[async_trait]
pub trait Extractor {
    async fn extract(&self, text: &str) -> anyhow::Result<ExtractionResponse>;
}
```

## Architecture

```
text
  │  chunking (chonkie)  ── Recursive (default) / Char (Python-parity) / Token
  ▼
LlmBackend.complete()   ── LlmsBackend (in-process `llms`) │ SdkAgentBackend (minimaxcc/glmcc/mimocc, stream-json) │ MockBackend
  ▼
parse  ── delimiter parser (Simple) │ JSON parser (SchemaJson)
  ▼
merge / dedup  ── entities by lowercased label, triples by (subj_id, predicate, obj_id)
  ▼
KnowledgeGraph { entities, triples }  ──►  JSON │ node-link │ Mermaid │ stats
```

- **Chunking** is delegated to the [`chonkie`](../chonkie) crate. The default
  `Recursive` strategy respects word/sentence boundaries; `Char` reproduces the
  Python `segment_chunks` character sliding window 1:1; `Token` bounds segments
  by real tiktoken tokens.
- **Backends** are pluggable via the `LlmBackend` trait:
  - `LlmsBackend` (feature `llms-backend`) — in-process [`llms`](../llms) crate;
    resolves any model string to the right provider (OpenAI-compatible, Ollama,
    Anthropic, …). Used for normal chat (Simple / SchemaJson).
  - `SdkAgentBackend` — drives a Claude-Code-wrapper agent CLI (`minimaxcc`
    default, or `glmcc` / `mimocc`) through the structured
    [`claude-agent-sdk-rs`](../claude-agent-sdk-rs) **stream-json** protocol,
    returning parsed messages instead of scraped stdout. Exposes a **native
    multi-turn session** (`open_session`), which the Simple engine uses to run
    gleaning + relation-gleaning as a real conversation. `pi-agent` (pi-rs) has a
    different CLI contract and keeps its own `PiAgentBackend`.
  - `MockBackend` — deterministic canned responses for tests/offline demos.

  Backends without a native conversation expose multi-turn through
  `ReplaySession`, which replays the accumulated history through `complete` each
  turn — so the Simple engine drives one transport-agnostic session loop.

## Schema modes

`SchemaJsonExtractor` and `ToolCallExtractor` are **schema-driven**: a `SchemaMode`
governs how a seed schema (lists of entity types, relation types and attributes)
constrains extraction. This axis is **orthogonal to the extraction mechanism** —
the same three modes apply whether the model emits JSON (SchemaJson) or calls tools
(ToolCall).

| Mode | Seed schema | The model may… | Use when |
|------|-------------|----------------|----------|
| `Open` *(default)* | not needed | infer any entity/relation types | exploring; no fixed ontology |
| `Fixed` | **required** | use **only** the schema's types | enforcing a closed, known ontology |
| `Evolving` | **required** | use the schema **and propose new types** | seeding a domain but allowing growth |

- **`Open`** ignores any schema and lets the model name types freely — the
  zero-config default.
- **`Fixed`** is closed-world, enforced by every engine (by different means):
  ToolCall JSON-Schema `enum`-constrains the tool arguments so the model *can't*
  emit out-of-schema types; SchemaJson and Agentic parse first, then **drop**
  whatever fell outside the schema (an entity by its type, a relation by its type
  or a dropped endpoint) and report it under `schema_dropped_records` /
  `schema_dropped_types`. Agentic additionally feeds the drop back into the next
  turn so a drifting model self-corrects.
- **`Evolving`** seeds the schema as guidance but lets the model add types;
  proposals are recorded under `new_schema_types` in the response metadata.

`Fixed` and `Evolving` **require a non-empty seed schema** — constraining to (or
evolving from) an *empty* schema is meaningless, so it is rejected with an error
rather than silently degrading. (It is the one degenerate cell of the
*schema-present × may-add-types* grid, made unrepresentable.)

Response metadata records both axes: `mode` = the engine (`schema_json` / `toolcall`)
and `schema_mode` = `open` / `fixed` / `evolving`.

A schema is a JSON object with `nodes` / `relations` / `attributes` arrays
(capitalised `Nodes` / `Relations` / `Attributes` are also accepted):

```json
{ "nodes": ["PERSON", "ORGANIZATION"], "relations": ["WORKS_AT"], "attributes": [] }
```

**Library** — `Open` needs no schema; `Fixed`/`Evolving` take one via the config.
`ToolCallExtractor` exposes the same `.schema_mode(…)` builder.

```rust
use kg_extract::extractor::{SchemaMode, SchemaJsonExtractor};
use kg_extract::types::{ExtractionConfig, Schema};

let open = SchemaJsonExtractor::new(backend);                 // Open (default)

let cfg = ExtractionConfig::from_schema(Schema::new(
    vec!["PERSON".into(), "ORGANIZATION".into()],        // nodes
    vec!["WORKS_AT".into()],                             // relations
    vec![],                                              // attributes
));
let fixed = SchemaJsonExtractor::with_config(backend, cfg)
    .schema_mode(SchemaMode::Fixed);                     // or SchemaMode::Evolving
```

**CLI** — `--schema-mode` selects the mode; `--schema` points at the JSON file
(required for `fixed`/`evolving`). Both flags apply to `-e schema-json` and `-e toolcall`:

```bash
kg-extract -e schema-json    -f doc.txt                                          # open (default)
kg-extract -e schema-json    --schema-mode fixed    --schema schema.json -f doc.txt
kg-extract -e toolcall --schema-mode evolving --schema schema.json -f doc.txt
```

### Spec vs execution

The schema, mode and dedup policy together form an `ExtractionSpec` — the
declarative *what*, separate from *execution* (model, chunking, segmentation).
`ExtractionConfig` embeds one (`config.spec`), so the two layers are explicit in
the types. The spec is serializable and reusable: **define it once and run it
through either engine** via `with_spec`:

```rust
use kg_extract::ExtractionSpec;
use kg_extract::extractor::{SchemaMode, ToolCallExtractor, SchemaJsonExtractor};
use kg_extract::types::Schema;

let spec = ExtractionSpec::new(
    Schema::new(vec!["PERSON".into()], vec!["WORKS_AT".into()], vec![]),
    SchemaMode::Fixed,
);
let via_json = SchemaJsonExtractor::with_spec(sj_backend, spec.clone());
let via_tools = ToolCallExtractor::with_spec(tool_backend, spec);  // same contract, different mechanism
```

## Presets (rich templates)

A flat `Schema` is only a type-vocabulary. A **preset** (template) is the richer
alternative: a multilingual (`zh`/`en`) description of the extraction target —
output field structure, a `guideline` (target persona + extraction rules), plus
identifier/display conventions. The crate ships a gallery of 37 presets across
six domains (`general` / `finance` / `legal` / `medicine` / `industry` / `tcm`),
**embedded into the binary** from `presets/**/*.yaml` (`include_dir`), so they
load by name with no external files. You can also bring your own template file in
the same YAML format.

When a preset is attached, the schema-driven prompt is rendered from the
template's guideline and fields (the [`template`](src/template) module), while
the output stays the same JSON contract the graph builder already parses — so the
template steers *what* is extracted, not the wire format.

```bash
# List the bundled presets (key  [type]  description)
kg-extract --list-presets

# Extract with a bundled preset (routes through the schema-json engine).
# Key is {domain}/{name}; a bare name resolves under general/.
kg-extract --preset general/concept_graph --lang en -b agent --agent minimaxcc -f doc.txt
kg-extract --preset graph -f doc.txt                      # == general/graph

# Bring your own template YAML (takes precedence over --preset)
kg-extract --preset-file my_template.yaml --lang zh -f doc.txt
```

```rust
use kg_extract::template::{gallery, TemplateCfg};
use kg_extract::{ExtractionSpec, SchemaJsonExtractor};

let tpl = gallery::get("general/concept_graph").unwrap();         // or TemplateCfg::from_yaml_file(path)
let spec = ExtractionSpec::from_template(tpl, Some("en".into())); // None = template's first language
let extractor = SchemaJsonExtractor::with_spec(backend, spec);
```

## Tool-calling mode

`ToolCallExtractor` exposes typed tools and lets the model **call** them instead
of emitting a blob we parse. Tool-call arguments are already structured JSON, so
parsing is essentially free. It shares the same [schema modes](#schema-modes):
in `Fixed` mode the entity/predicate tool args are JSON-Schema `enum`-constrained
to the seeded schema; `Open` (default) and `Evolving` leave them free-form.

| Tool | Purpose |
|------|---------|
| `add_entity(name, type, description, attributes)` | record an entity |
| `add_relation(source, predicate, target, description, strength)` | record a relationship |
| `add_attribute(entity, key, value)` | attach an attribute to an entity |
| `propose_schema_type(kind, name, reason)` | suggest a new schema type (schema evolution) |
| `list_entities()` | read already-recorded entities (multi-round only) |
| `finish()` | signal completion |

Execution is **single-round collection** by default (`max_rounds = 1`): one
request, gather every tool call from that response, build the graph. Set
`max_rounds > 1` for a bounded agentic loop where tool results (including
`list_entities`) are fed back so the model can avoid dangling relations.
`.schema_mode(SchemaMode::Evolving)` (seed schema required) drops the enum
constraints and records `propose_schema_type` calls into `new_schema_types`
metadata.

Requires a tool-capable backend (`LlmsBackend`; the agent-CLI backend does not
expose function calling). Relations reference entity *names*, resolved at build
time; dangling endpoints are dropped.

```rust
use kg_extract::extractor::{Extractor, ToolCallExtractor};
let extractor = ToolCallExtractor::new(backend);          // single-round, qwen-max
let response = extractor.extract("OpenAI built GPT-4.").await?;
```

## Provenance citations (feature `citations`)

Built with `--features citations`, every extracted entity and triple carries
**where it came from**: a `citations` array in its `metadata`, each entry
`{"doc": <name|null>, "lines": [start, end]}` (1-based, inclusive).

```bash
cargo build --release --features citations
kg-extract -e agentic --agent minimaxcc -f doc.md -o json   # records now carry metadata.citations
```

Line ranges are computed **by the code, never by the model**: the chunker
already tracks each chunk/slice's char offsets, and a per-document line index
maps offsets to lines — so citations cannot be hallucinated. Granularity
follows the engine: `simple`/`agentic` cite the chunk/slice containing the
mention (agentic relation-gleaning records, which look at the whole document,
cite the full range); single-shot `schema-json`/`toolcall` cite the whole
document. The CLI sets the doc name from `-f` (stdin → `null`); with
`--input-format chunks` it comes from the chunks' `metadata.source` instead,
and line ranges from their `metadata.start_line`/`end_line` (see
[Pre-chunked input](#pre-chunked-input---input-format-chunks)). Library users
set `config.source_doc`.

A record seen in several places accumulates **multiple citations**: slice-level
recurrences union within the agentic session, and every dedup/merge path
(`merger`, all `MergeStrategy` variants, duplicate-triple drops) unions the two
sides' citations — so merging graphs from different documents yields entities
citing every document they appeared in. Without the feature, output is
byte-for-byte unchanged and the bookkeeping is compiled out.

## Types

`EntityType` (122 variants) and `PredicateType` (108 variants) are enums whose
string values are SCREAMING_SNAKE_CASE (`EntityType::Person` ⇄ `"PERSON"`),
ported verbatim from the Python enums. `KnowledgeGraph` keeps entities in an
insertion-ordered map so Mermaid/merge output is stable.

## Library usage

```rust
use std::sync::Arc;
use kg_extract::backend::LlmsBackend;             // requires feature `llms-backend`
use kg_extract::extractor::{Extractor, SimpleExtractor};

# async fn run() -> anyhow::Result<()> {
let backend = Arc::new(LlmsBackend::new());
let extractor = SimpleExtractor::new(backend);    // qwen-max, recursive chunking
let response = extractor.extract("OpenAI developed GPT-4 using transformers.").await?;

println!("{} entities, {} triples", response.num_entities(), response.num_triples());
println!("{}", response.get_mermaid_code());
# Ok(()) }
```

`SchemaJsonExtractor` and `ToolCallExtractor` are **schema-driven** — see
[Schema modes](#schema-modes) for `Open` / `Fixed` / `Evolving` and how to seed
a schema.

## CLI

```bash
# Build (mock/agent backends only)
cargo build
# Build with the in-process llms backend
cargo build --features llms-backend

# Simple engine via llms, emit Mermaid
echo "OpenAI developed GPT-4." | kg-extract -e simple -b llms -o mermaid

# SchemaJson open mode (no schema) — the default
kg-extract -e schema-json -b agent --agent minimaxcc -f doc.txt
# SchemaJson evolving mode: seed a schema, let the model extend it
kg-extract -e schema-json --schema-mode evolving --schema schema.json -b agent --agent minimaxcc -f doc.txt
```

| Flag | Meaning |
|------|---------|
| `-e, --engine` | `simple` \| `schema-json` \| `toolcall` \| `agentic` |
| `-b, --backend` | `llms` \| `agent` \| `mock` (ignored by `-e agentic`, which always drives the SDK) |
| `--agent` | agent CLI for `-b agent` / `-e agentic`: `minimaxcc` (default) \| `glmcc` \| `mimocc` |
| `-c, --chunker` | `recursive` (default) \| `char` \| `token` |
| `-m, --model` | override the engine's default model |
| `--schema-mode` | schema-json/toolcall/agentic: `open` (default) \| `fixed` \| `evolving` (agentic enforces only `fixed`) |
| `--schema` | schema JSON file (required for `fixed`/`evolving`; agentic validates each slice against it under `fixed`) |
| `--preset` | bundled preset by key (`general/concept_graph`, or bare `graph`); routes through schema-json |
| `--preset-file` | your own template YAML (takes precedence over `--preset`) |
| `--lang` | language to render the preset/template (`zh` \| `en` \| …; default = template's first) |
| `--list-presets` | print the bundled presets and exit |
| `--max-rounds` | tool-call rounds (1 = single-round, default) |
| `--relation-gleaning` | simple/agentic: targeted rounds that re-question orphan entities to recover edges (0 = off) |
| `--mock-tool-calls` | mock backend only: scripted tool calls for `-e toolcall` offline/e2e tests |
| `-F, --input-format` | `text` (default) \| `chunks` — input is chonkie chunk JSON/JSONL, consumed without re-chunking |
| `-o, --output` | `json` (default) \| `node-link` \| `ladybug-import` \| `mermaid` \| `stats` |

### LadybugDB import output

`-o ladybug-import` emits the JSON format accepted by the sibling
[`graphdb-ladybug`](../graphdb-ladybug) CLI:

```bash
kg-extract -e schema-json -b agent --agent minimaxcc -f doc.md -o ladybug-import > kg.lbug.json
lbug /tmp/kg-doc import kg.lbug.json --create-tables
lbug /tmp/kg-doc query "MATCH (a:KgEntity)-[r:DEVELOPED_BY]->(b:KgEntity) RETURN a.label, r.predicate, b.label;"
```

The export uses one generic node table (`KgEntity`) and one relationship table
per extracted predicate. Entity and relation metadata are stored as JSON strings
so the import stays compatible with Ladybug's scalar property binding.

For a deterministic local smoke test that avoids live LLM calls:

```bash
just ladybug-smoke
```

For a broader fact-coverage check, run the fixture evaluator. It imports the
same Markdown facts through six deterministic variants — `simple`,
`schema-json`, and `toolcall`, each with chunked/plain input — then queries
Ladybug for every expected relationship:

```bash
just ladybug-eval
just ladybug-eval medium_product
```

To append one real agent extraction pass on the same fixture:

```bash
just ladybug-eval-live minimaxcc
just ladybug-eval-live minimaxcc medium_product
```

To test the multi-turn `agentic` extractor instead:

```bash
just ladybug-eval-agentic minimaxcc
just ladybug-eval-agentic minimaxcc medium_product
```

To run the full local loop — deterministic variants, live `schema-json`, live
`agentic`, and an agent that judges the Ladybug query evidence:

```bash
just ladybug-eval-full-verify minimaxcc
just ladybug-eval-full-verify minimaxcc medium_product
```

### Pre-chunked input (`--input-format chunks`)

If the text was already chunked — e.g. by the sibling [`chonkie`](../chonkie)
crate — feed the chunks straight in instead of letting kg-extract re-chunk:

```bash
# chonkie cuts doc.md into chunks; kg-extract extracts per given chunk.
chonkie --jsonl --chunker recursive --chunk-size 512 -f doc.md \
  | kg-extract -F chunks -e simple -b llms
```

Accepted shapes: a JSON array (`chonkie --json`), JSONL (`chonkie --jsonl`,
whose trailing `{"truncated": ...}` metadata line is skipped), or the
`{"chunks": [...]}` truncation wrapper. Each chunk needs a `text` field;
`start_index`/`end_index` and `metadata.{source,start_line,end_line}` are used
when present.

Engine behaviour:

- **`simple` / `agentic`** (the chunking engines): the given chunks ARE the
  chunks/slices — no internal segmentation, no `min_segment_size` filtering.
  `--chunker` and segment sizing are ignored.
- **`schema-json` / `toolcall`** (single-shot engines): the chunk texts are
  joined (`\n\n`) and extracted in one call, exactly as for plain text.

With `--features citations`, provenance comes from the chunks themselves:
`metadata.source` names the cited document (the *original* file the chunks
were cut from — `-f` names the chunks file, so it is not used as the doc) and
`metadata.start_line`/`end_line` become each record's cited line range. Chunks
without line metadata (e.g. `chonkie --no-lines`) yield unstamped records.

### Agentic engine (`-e agentic`)

An alternative to `simple` for **coherence-critical, small-to-medium** documents.
Instead of extracting each chunk in its own session and merging, it drives the
**whole document through one continuous multi-turn SDK session**: the full text is
written to `document.md` in an isolated temp dir, the agent runs there in a
**read-only tool sandbox** (Read/Grep/Glob; no Write/Edit/Bash), and slices are
fed as turns. Because one conversation carries the context, the model **reuses
entity names across slices** (coreference at extraction time), and the final
[relation-gleaning](#relation-gleaning) pass connects orphans **across** slices,
not just within a chunk.

```bash
kg-extract -e agentic --agent minimaxcc --relation-gleaning 2 -f doc.txt -o mermaid
```

Trade-offs vs `simple` (measured on a 62 KB manual):

| | `simple` (per-chunk, concurrent) | `agentic` (one session, serial) |
|---|---|---|
| entities | 516 (heavily fragmented — `在线课堂` split into ~7 nodes) | 136 (consolidated — one node) |
| orphan rate | ~1 % | ~0 % |
| speed | fast (chunks run concurrently) | slower (strictly sequential) |
| recall | higher (more granular, more redundant) | slightly lower (a few minor entities merged away) |

So `agentic` is a **consolidation / coreference** strategy, not a recall one — its
extraction-time deduplication is *not* reproducible by a post-hoc merge pass
(`--merge-strategy llm` only trims ~20 % of the fragments). The self-serve `grep`
context the sandbox enables is rarely used in practice (in-order slices already
carry context in the conversation); it matters mainly for cross-section lookups.

#### Schema modes (`--schema-mode`)

The agentic engine honours all three [schema modes](#schema-modes), each a
different use of the multi-turn session:

- **`open`** (default) — schema is hints only; the model extracts freely.
- **`fixed`** — closed-world validation loop (below): out-of-schema records are
  dropped and the model is corrected mid-conversation.
- **`evolving`** — seed types are *preferred* but nothing is dropped; the types
  the model uses **outside** the seed are recorded as `new_schema_types` in the
  response metadata (and summarised on stderr), mirroring SchemaJson/ToolCall.

Same rich paragraph, seed `{nodes:[Person,Company], relations:[WORKS_FOR]}`:

| mode | entity types in graph | new types recorded |
|---|---|---|
| `open` | 8 (free) | — |
| `evolving` | 6 (all kept) | City, Location, Pet, Product, Computing_Platform, Based_In, Lives_In, Owns, … |
| `fixed` | **2** (Person, Company) | — (dropped instead) |

##### Fixed-schema enforcement (`--schema-mode fixed`)

`--schema-mode fixed --schema schema.json` turns the session into a per-slice
validation loop:

1. The system prompt states the closed type vocabulary (`STRICT SCHEMA — use ONLY…`).
2. After each slice, every extracted entity/relation is checked against the schema.
   Out-of-schema records are **dropped** (an entity by its type, a relation by its
   type *or* a dropped endpoint), and the dropped type names are recorded.
3. If anything was dropped, the **next slice's turn carries a correction** ("I
   discarded N records, stay within these types"), so a drifting model
   re-anchors instead of compounding the error.
4. If a whole slice comes back *entirely* out-of-schema (the degenerate case),
   it is **re-done once** with a sterner reminder before moving on.

Validation is on the **raw type token**, so a domain-specific schema type outside
the built-in `EntityType` vocabulary (which the enum would collapse to `Other`)
still matches. `schema_dropped_records` / `schema_dropped_types` land in the
response metadata for auditing.

```bash
kg-extract -e agentic --agent minimaxcc --schema-mode fixed --schema schema.json -f doc.txt -o json
```

In practice the strict prompt does most of the work — a compliant model rarely
emits out-of-schema records, so the drop/feedback loop is mostly a **safety net**.
Its value grows with document length (more slices = more chances to drift) and
with stricter/smaller schemas.

```bash
# Tool-calling engine via llms (requires --features llms-backend); open by default
kg-extract -e toolcall -b llms -f doc.txt -o json
# Fixed: enum-constrain tool args to a seeded schema
kg-extract -e toolcall -b llms --schema-mode fixed --schema schema.json -f doc.txt -o json
# Agentic multi-round, evolving schema
kg-extract -e toolcall -b llms --schema-mode evolving --schema schema.json --max-rounds 4 -f doc.txt
```

## Parity notes

This is a behavioural port of the Python original, including a couple of its
quirks (documented in code): SimpleExtractor's relationship tuple field-shift
(the relationship-type token drives predicate inference), and the
exact-match-before-alias entity typing.

## Dev

```bash
cargo test
cargo clippy --all-targets
```
