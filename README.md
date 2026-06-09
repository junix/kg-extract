# kg-extract

Multi-strategy **knowledge-graph extraction** in Rust — a faithful port of the
Python `graph.kg_extractor` module. Turns unstructured text into a
`KnowledgeGraph` of typed entities and predicate-typed triples, using one of
three extraction strategies behind a common trait.

## Strategies

| Extractor | Approach | Default model |
|-----------|----------|---------------|
| `SimpleExtractor` | General LLM chat with GraphRAG-style **delimiter prompting** + **multi-gleaning** (iteratively asks "what did you miss?" for high recall) | `qwen-max` |
| `TriplexExtractor` | **NER + triple** extraction via a Triplex-style model, **segmenting** large inputs and merging per-segment graphs | `sciphi/triplex:latest` (Ollama) |
| `YoutuExtractor` | **Schema-driven** JSON extraction with optional **agent mode** (schema evolution) and **community detection** | `qwen-max` |
| `ToolCallExtractor` | **Tool / function calling** — typed `add_entity` / `add_relation` / … tools; structured by construction, **no output parsing** | `qwen-max` |

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
parse  ── delimiter parser (Simple) │ JSON parser (Triplex/Youtu)
  ▼
merge / dedup  ── entities by lowercased label, triples by (subj_id, predicate, obj_id)
  ▼
KnowledgeGraph { entities, triples }  ──►  JSON │ Mermaid │ stats
```

- **Chunking** is delegated to the [`chonkie`](../chonkie) crate. The default
  `Recursive` strategy respects word/sentence boundaries; `Char` reproduces the
  Python `segment_chunks` character sliding window 1:1; `Token` bounds segments
  by real tiktoken tokens.
- **Backends** are pluggable via the `LlmBackend` trait:
  - `LlmsBackend` (feature `llms-backend`) — in-process [`llms`](../llms) crate;
    resolves any model string to the right provider (OpenAI-compatible, Ollama,
    Anthropic, …). Used for normal chat (Simple / Triplex / Youtu noagent).
  - `AgentCliBackend` — subprocess to a Claude-Code-wrapper agent CLI
    (`minimaxcc` default, or `glmcc` / `mimocc`) in headless `-p` mode. Intended
    for Youtu **agent** mode, where schema-evolving extraction is genuinely
    agentic.
  - `MockBackend` — deterministic canned responses for tests/offline demos.

## Tool-calling mode

`ToolCallExtractor` exposes typed tools and lets the model **call** them instead
of emitting a blob we parse. Tool-call arguments are already structured JSON, so
parsing is essentially free, and JSON-Schema `enum`s constrain entity/predicate
types to the configured schema.

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
`schema_evolution(true)` drops the enum constraints and records
`propose_schema_type` calls into `new_schema_types` metadata.

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

Youtu with agent mode + community detection, driven by an agent CLI:

```rust
use kg_extract::backend::{AgentCli, AgentCliBackend};
use kg_extract::extractor::{Extractor, YoutuExtractor, YoutuMode};

let backend = Arc::new(AgentCliBackend::new(AgentCli::Minimaxcc));
let extractor = YoutuExtractor::new(backend)
    .mode(YoutuMode::Agent)
    .community_detection(true);
let response = extractor.extract(text).await?;
```

## CLI

```bash
# Build (mock/agent backends only)
cargo build
# Build with the in-process llms backend
cargo build --features llms-backend

# Simple engine via llms, emit Mermaid
echo "OpenAI developed GPT-4." | kg-extract -e simple -b llms -o mermaid

# Triplex via Ollama (sciphi/triplex), JSON output
kg-extract -e triplex -b llms -f doc.txt -o json

# Youtu agent mode + community detection, driven by minimaxcc
kg-extract -e youtu --youtu-agent --community -b agent --agent minimaxcc -f doc.txt
```

| Flag | Meaning |
|------|---------|
| `-e, --engine` | `simple` \| `triplex` \| `youtu` \| `toolcall` |
| `-b, --backend` | `llms` \| `agent` \| `mock` |
| `--agent` | agent CLI for `-b agent`: `minimaxcc` (default) \| `glmcc` \| `mimocc` |
| `-c, --chunker` | `recursive` (default) \| `char` \| `token` |
| `-m, --model` | override the engine's default model |
| `--youtu-agent` | Youtu schema-evolution mode |
| `--community` | enable community detection (Youtu) |
| `--toolcall-agent` | tool-call schema-evolution mode |
| `--max-rounds` | tool-call rounds (1 = single-round, default) |
| `-o, --output` | `json` (default) \| `mermaid` \| `stats` |

```bash
# Tool-calling engine via llms (requires --features llms-backend)
kg-extract -e toolcall -b llms -f doc.txt -o json
# Agentic multi-round with schema evolution
kg-extract -e toolcall -b llms --toolcall-agent --max-rounds 4 -f doc.txt
```

## Parity notes

This is a behavioural port of the Python original, including a couple of its
quirks (documented in code): SimpleExtractor's relationship tuple field-shift
(the relationship-type token drives predicate inference), and the
exact-match-before-alias entity typing. Community detection uses dependency-free
**label propagation** in place of networkx greedy-modularity.

## Dev

```bash
cargo test
cargo clippy --all-targets
```
