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

`to_tuple = (subject.id, predicate.predicate_type.value, object.id)` is the
**dedup key** — the middle component is the *normalized* predicate value, never
the raw surface token, so `"uses"` and `"USES"` (or `"founded by"` vs
`"FOUNDED_BY"`) collapse to one edge across chunks.
Two triples with the same tuple are considered identical for deduplication.
[T] (`graph::merge_dedups_identical_triples_within_other`,
`graph::merge_dedups_surface_variant_predicates_across_chunks`)

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
| `fuzzy` | + label normalized by: lowercase, non-alphanumeric→space, drop leading article, strip trailing corporate-suffix token; + near-identical (similarity ≥ 0.85 over Unicode chars) AND type-compatible, for normalized labels of length ≥ 6; + token-set overlap (see below) AND type-compatible |

Normalization rules (observable): `"Open AI" → "open ai"`,
`"Anthropic, PBC" → "anthropic"`, `"Google Inc." → "google"`,
`"The New York Times" → "new york times"`, a bare suffix token (e.g. `"Inc"`)
is **never** stripped to empty. [T] (`merger::normalize_label_strips_punctuation_articles_and_suffixes`)

Fuzzy never fuses normalized labels shorter than 6 characters **via edit
distance**. Two types are compatible if equal or either is `Other`. On a tie
the earliest-inserted entity wins (deterministic, independent of map
iteration order).
[T] (`merger::coref_fuzzy_merges_normalized_variant_and_remaps_triples`,
`merger::coref_fuzzy_merges_near_typo_but_respects_type_and_length`)

The **token-set channel** catches surfaces edit distance cannot — reorderings,
dropped/added qualifier tokens, and containment. Over the normalized token
sets, a pair is coreferent when (a) the smaller set is fully contained in the
larger **and has ≥ 2 tokens** (score 1.0 — single-token containment like
`"Apple"` ⊂ `"Apple Store"` is rejected as too generic), or (b) the Jaccard
similarity |∩|/|∪| is **≥ 0.6**. The channel has no length gate (short names
cannot reach near-total token overlap accidentally) but keeps the
type-compatibility gate. When both channels match, the higher score wins;
ties still resolve to the earliest-inserted entity.
[T] (`merger::token_set_similarity_subset_and_jaccard`,
`merger::coref_fuzzy_token_set_merges_subset_and_jaccard_surfaces`,
`merger::coref_fuzzy_token_set_respects_type_gate_and_short_names`,
`merger::coref_fuzzy_token_set_is_deterministic_earliest_wins`)

The corporate-suffix and leading-article token lists are
**implementation-defined**; the contract is the normalization observable
above plus the length/type/similarity gates.

## 8.8 Segment

```
Segment {
  content: string
  index: int
  start, end: int                  // char offsets in the combined input
  range: SourceRange?              // char_span, line, page, bbox
  title: string?                   // pre-chunked payload (kg-multimodal's item name)
  metadata: map<string, json>      // pre-chunked payload (e.g. mm_* keys); empty for plain text
}
```

`Segment.range` replaces the former public `Segment.lines` field. This is a
**Rust source-breaking** API change for callers that construct `Segment`
literals or access the old field. The pre-chunked JSON wire remains compatible:
it already carries the optional protocol `range`, and adding optional page/bbox
coordinates does not invalidate line-only inputs. `title`/`metadata` are a
later additive change with the same profile: struct literals need the new
fields, the wire format only grew optional keys.

Plain-text extraction uses the char offsets to derive a line span. Pre-chunked
parsing retains the complete supplied range on the segment. When records are
stamped, page/bbox selects the rich citation form and preserves that complete
range; otherwise only the line span is serialized in the legacy form (the char
span remains segment-local positioning data). The `title`/`metadata` payload is
stamped alongside: chunk-aware engines attach it to every record extracted from
that segment as the `chunk_title` / `chunk_metadata` metadata keys (§9.5),
which the protocol projection passes through to properties.

## 8.9 Citation

```
Citation { doc: string?, range: SourceRange }
```

The public fields `start_line` / `end_line` were replaced by `range`; direct
field access and struct literals are therefore Rust source-breaking. `Citation`
also no longer implements `Eq` because `SourceRange.bbox` uses floating-point
coordinates. The `Citation::new(doc, start_line, end_line)` constructor remains
source-compatible for valid 1-based ranges. Zero or inverted ranges now fail
`LineSpan` validation and do not serialize as a legacy `{doc,lines}` entry.

Stored under `metadata["citations"]` as a JSON array with two accepted shapes:

- line-only provenance keeps the legacy wire shape
  `{"doc": <name|null>, "lines": [start, end]}`;
- a citation carrying page or bbox uses
  `{"doc": <name|null>, "range": <SourceRange>}` and preserves the supplied
  char/line/page/bbox coordinates together.

A record seen in several places carries several citations; merging unions them
(duplicates skipped by value equality). [T]
(`citation::attach_deduplicates_identical_citations`,
`citation::union_merges_distinct_and_skips_duplicate_citations`)

`KnowledgeGraph::to_kg_document` accepts an array only when every item matches
one of these citation shapes, promotes each to first-class `KgEvidence.range`,
and removes the recognized `citations` key from entity and relation
`properties`. A malformed, foreign, or mixed array is preserved verbatim as a
property and is not partially promoted. Protocol output therefore avoids both
duplicate provenance and silent user-data loss. [T]
(`protocol::knowledge_graph_converts_to_portable_kg_protocol`,
`protocol::foreign_citation_object_list_is_not_promoted_or_dropped`,
`protocol::mixed_internal_and_foreign_citations_are_preserved_as_user_metadata`,
`protocol::rich_citation_with_foreign_nested_range_field_is_preserved`,
`protocol::legacy_citation_with_extra_line_value_is_preserved`,
`simple::prechunked_multimodal_range_survives_as_entity_and_relation_evidence`)

## 8.10 Extraction response metadata keys

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

## 8.11 Community detection mapping (feature `community`)

`to_community_graph` maps a `KnowledgeGraph` onto `kg_community::Graph`:
entities become nodes in insertion order; **each triple becomes one undirected
edge of weight 1.0, and edges are NOT deduplicated** — `kg-community` sums
parallel edges, so N triples between the same pair act as an edge of weight N
(KG edge multiplicity is a community-strength signal and must survive the
adapter). Self-loops and triples with unregistered endpoints are skipped.
`detect_communities*` returns a stable `entity_id → community` map;
`communities_json` renders `{num_communities, communities: {"0": [ids…]}}`
with ascending community keys and sorted member ids (deterministic).
[T] (`community::multiplicity_produces_parallel_weighted_edges`,
`community::multiplicity_weights_change_the_partition`,
`community::communities_json_groups_members_by_community`)
