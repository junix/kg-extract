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
LlmBackend.complete()   ── LlmsBackend (in-process `llms`) │ AgentCliBackend (minimaxcc/glmcc/mimocc) │ MockBackend
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
  - `AgentCliBackend` — subprocess to a Claude-Code-wrapper agent CLI
    (`minimaxcc` default, or `glmcc` / `mimocc`) in headless `-p` mode. Intended
    for SchemaJson **evolving** mode, where schema-evolving extraction is genuinely
    agentic.
  - `MockBackend` — deterministic canned responses for tests/offline demos.

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
- **`Fixed`** is closed-world: SchemaJson prompts "use only these types"; ToolCall
  JSON-Schema `enum`-constrains the tool arguments to them.
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
| `-e, --engine` | `simple` \| `schema-json` \| `toolcall` |
| `-b, --backend` | `llms` \| `agent` \| `mock` |
| `--agent` | agent CLI for `-b agent`: `minimaxcc` (default) \| `glmcc` \| `mimocc` |
| `-c, --chunker` | `recursive` (default) \| `char` \| `token` |
| `-m, --model` | override the engine's default model |
| `--schema-mode` | schema-json/toolcall: `open` (default) \| `fixed` \| `evolving` |
| `--schema` | schema-json/toolcall schema JSON file (required for `fixed`/`evolving`) |
| `--preset` | bundled preset by key (`general/concept_graph`, or bare `graph`); routes through schema-json |
| `--preset-file` | your own template YAML (takes precedence over `--preset`) |
| `--lang` | language to render the preset/template (`zh` \| `en` \| …; default = template's first) |
| `--list-presets` | print the bundled presets and exit |
| `--max-rounds` | tool-call rounds (1 = single-round, default) |
| `-o, --output` | `json` (default) \| `node-link` \| `mermaid` \| `stats` |

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
