# 02 — Data Model

## 8.1 Knowledge graph

```
KnowledgeGraph {
  entities: ordered map id → Entity   (insertion-ordered; order is stable across emit/merge)
  triples: ordered list of Triple
  metadata: map string → JSON value
}
```

Invariants (enforced in code and by tests):

- The entity map is **insertion-ordered**. Emitting (Mermaid, node-link,
  to_dict) and merging preserve this order. `KnowledgeGraph::merge` does not
  sort. [T] (`graph::to_node_link_uses_source_target_referencing_node_ids`)
- `add_triple(T)` re-inserts both endpoint snapshots into the entity table,
  but **never loses provenance**: citations already on a stored entity are
  unioned into the incoming snapshot before it overwrites.
  [T] (`citation::add_triple_endpoint_overwrite_keeps_entity_citations`)

## 8.2 Entity

```
Entity {
  id: string                         // "entity_" ++ md5(name)[0..8], raw name bytes
  label: string                      // the surface name (case preserved per engine rule)
  entity_type: EntityType            // canonical enum value (normalized)
  raw_type: string?                  // the literal type token the model emitted (lossless)
  confidence: float?                 // [0,1]; absent = unknown
  description: string?
  metadata: map string → JSON value  // includes attributes and "citations"
}
```

Invariants:

- `output_type = raw_type` when `raw_type` is non-empty, else
  `entity_type.value()`. This is the type exposed in all open-schema output;
  `entity_type.value()` is the normalized (enum) form exposed separately as
  `normalized_type`. [T] (`schema_json::open_schema_output_preserves_raw_entity_and_relation_types`)
- `id` is `entity_<md5(name)[0..8]>` over the raw name bytes (case-sensitive),
  shared by all engines and the MCP store. [T] (`simple::entity_id_is_deterministic_md5`,
  `schema_json::entity_ids_are_deterministic_md5`)

`EntityType` is a fixed vocabulary whose string values are
SCREAMING_SNAKE_CASE. Resolution of a free-form token:

```
resolve(token):
  n ← trim(token); upper(n); replace {' ', '-'} with '_'
  exact match in vocabulary  → (variant, Exact)
  alias table match          → (aliased variant, Aliased)
  otherwise                  → (OTHER, Fallback)
```

`TypeMatch ∈ {Exact, Aliased, Fallback}` is auditable: a graph whose tokens
were aliased or fell back carries a `type_normalization` report (sorted keys)
in the response metadata; a graph where every token resolved exactly has no
such key. [T] (`entity::resolve_reports_match_kind`,
`graph::type_normalization_report_records_aliased_and_fallback`)

The vocabulary size and the alias table contents are **implementation-defined**
(exact reproduction of the Python port's enum); the contract is only the
resolution precedence above and that the report distinguishes the three
outcomes.

## 8.3 Triple

```
Triple {
  subject: Entity
  predicate: Predicate
  object: Entity
  confidence: float?               // [0,1]; the relation strength
  metadata: map string → JSON value
}
```

`to_tuple = (subject.id, predicate.output_type, object.id)` is the **dedup key**.
Two triples with the same tuple are considered identical for deduplication.
[T] (`graph::merge_dedups_identical_triples_within_other`)

### Predicate

```
Predicate {
  predicate_type: PredicateType    // canonical enum (normalized), e.g. DEVELOPED_BY
  label: string?                   // the literal relation string the model emitted
  raw_type: string?                // lossless raw token (Simple engines reuse label)
  confidence: float?
  metadata: map string → JSON value
}
```

`output_type = raw_type` if non-empty else `label` if non-empty else
`predicate_type.value()`. Resolution `PredicateType::resolve` follows the same
`upper / '-'→'_' / exact / alias / fallback (RELATED_TO)` precedence as
`EntityType`.

## 8.4 Schema

```
Schema {
  nodes:       ordered list of string   // entity type names
  relations:   ordered list of string   // predicate type names
  attributes:  ordered list of string   // allowed attribute keys (MCP store)
  metadata:    map string → JSON value
}
```

- `is_empty = nodes.empty ∧ relations.empty ∧ attributes.empty`.
- JSON accepts both capitalized (`Nodes`/`Relations`/`Attributes`) and
  lowercase keys. [T] (schema.rs `from_json_str`)
- `merge(other)` set-unions each list (BTreeSet-sorted output).

### SchemaMode

Three of the four cells of `(schema present?) × (may add types?)`:

| Mode | Schema | Model MAY add types | Engines |
|------|--------|---------------------|---------|
| `Open` *(default)* | ignored | yes (free) | all four |
| `Fixed` | **required non-empty** | **no** | schema-json, toolcall, agentic (validation) |
| `Evolving` | **required non-empty** | yes (recorded) | schema-json, toolcall, agentic |

The fourth cell — constrain to an *empty* schema — is **unrepresentable**:
`Fixed`/`Evolving` with an empty schema is rejected with an error at
`extract` time. [T] (`schema_json::fixed_mode_without_schema_errors`,
`toolcall::fixed_mode_without_schema_errors`)

## 8.5 Extraction config / spec

```
ExtractionSpec {                       // the declarative "what"
  schema:          Schema
  mode:            SchemaMode = Open
  merge_duplicates:bool = true
  merge_strategy:  MergeStrategy = keep-existing
  coref:           CorefMode = off
  template:        TemplateCfg?        // rich preset (schema-json prompt source)
  language:        string?             // render language for a template
}

ExtractionConfig {                     // spec + the execution "how"
  spec:             ExtractionSpec
  segment_size, overlap, min_segment_size: int
  model_name:       string             // default "qwen-max"
  chunker:          ChunkStrategy = recursive
  max_concurrency:  int                // cooperative; 1 = sequential
  source_doc:       string?            // doc name for citations
}
```

The spec is serializable and reusable: the **same** spec runs through either
the schema-json or the toolcall engine. [T] (`schema_json::one_spec_runs_through_both_engines`)

## 8.6 MergeStrategy

How two entities judged identical (same key — see 8.7) are combined:

| Strategy | Confidence | Description | Type | Metadata | id |
|----------|-----------|-------------|------|----------|----|
| `keep-existing` *(default)* | keep existing | keep existing | keep existing | keep existing | existing |
| `keep-incoming` | incoming | incoming | incoming | incoming | existing |
| `field-union` | `max(a,b)` | longer (trimmed char count) | specific beats `Other` | union, existing wins on conflict | existing |
| `llm` | as field-union | LLM-synthesised when both descriptions differ & non-empty, else field-union | as field-union | as field-union | existing |

Whichever fields are kept, **citations from BOTH sides always survive**
(unioned). [T] (`merger::field_union_combines_description_confidence_metadata_and_type`,
`merger::keep_incoming_replaces_but_preserves_canonical_id`,
`merger::llm_strategy_synthesizes_merged_description`)

`llm` degrades to `field-union` when no backend is available or the synthesis
call fails.

## 8.7 CorefMode (duplicate recognition)

Orthogonal to MergeStrategy. Decides which entities count as "the same":

| Mode | Recognizes as same |
|------|--------------------|
| `off` *(default)* | exact case-insensitive label only |
| `fuzzy` | + label normalized by: lowercase, non-alphanumeric→space, drop leading article, strip trailing corporate-suffix token; + near-identical (similarity ≥ 0.85 over Unicode chars) AND type-compatible, for normalized labels of length ≥ 6 |

Normalization rules (observable): `"Open AI" → "open ai"`,
`"Anthropic, PBC" → "anthropic"`, `"Google Inc." → "google"`,
`"The New York Times" → "new york times"`, a bare suffix token (e.g. `"Inc"`)
is **never** stripped to empty. [T] (`merger::normalize_label_strips_punctuation_articles_and_suffixes`)

Fuzzy never fuses normalized labels shorter than 6 characters. Two types are
compatible if equal or either is `Other`. On a tie the earliest-inserted
entity wins (deterministic, independent of map iteration order).
[T] (`merger::coref_fuzzy_merges_normalized_variant_and_remaps_triples`,
`merger::coref_fuzzy_merges_near_typo_but_respects_type_and_length`)

The corporate-suffix and leading-article token lists are
**implementation-defined**; the contract is the normalization observable
above plus the length/type/similarity gates.

## 8.8 Citation

```
Citation { doc: string?, start_line: int (1-based), end_line: int (1-based, inclusive) }
```

Stored under `metadata["citations"]` as a JSON array of
`{"doc": <name|null>, "lines": [start, end]}`. A record seen in several
places carries several citations; merging unions them (duplicates skipped by
value equality). [T] (`citation::attach_deduplicates_identical_citations`,
`citation::union_merges_distinct_and_skips_duplicate_citations`)

## 8.9 Extraction response metadata keys

| Key | Meaning | Produced by |
|-----|---------|-------------|
| `mode` | engine name: `schema_json` / `toolcall` | schema-json, toolcall |
| `schema_mode` | `open` / `fixed` / `evolving` | schema-json, toolcall |
| `schema_used` | the seed schema actually applied | schema-json |
| `new_schema_types` | `{nodes,relations,attributes}` proposed | schema-json (Evolving), toolcall (Evolving) |
| `schema_dropped_records` | int count removed by Fixed enforcement | schema-json (Fixed) |
| `schema_dropped_types` | list of type names removed | schema-json (Fixed) |
| `model` | model name | schema-json |
| `type_normalization` | alias/fallback audit (present only when non-trivial) | CLI annotation |
