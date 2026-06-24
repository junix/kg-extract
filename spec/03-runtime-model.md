# 03 — Runtime Model

## 9. Engine selection

Four engines implement `Extractor::extract(text) → ExtractionResponse`:

| Engine | Mechanism | Chunks? | Multi-turn | Schema modes |
|--------|-----------|---------|------------|--------------|
| Simple | delimiter prompt → parse | yes (concurrent per-chunk) | per-chunk session (gleaning + relation-gleaning) | Open (schema as prompt hint) |
| SchemaJson | JSON prompt → parse | no (single-shot) | one call | Open / Fixed / Evolving |
| ToolCall | tool/function calls | no (single-shot or multi-round) | bounded rounds | Open / Fixed / Evolving |
| Agentic | SDK read-only sandbox, one session over the whole document | slices as turns | one long session | Open / Fixed / Evolving |

Pre-chunked input (`extract_prechunked`):

- Simple / Agentic (chunking engines): the given chunks ARE the segments — no
  re-chunking, no `min_segment_size` filtering. Empty input is an error.
  [T] (`simple::prechunked_extracts_each_given_chunk_without_rechunking`,
  `simple::prechunked_empty_input_errors`)
- SchemaJson / ToolCall (single-shot engines): chunk texts are joined with
  `\n\n` and extracted in one call, exactly like plain text.
  [T] (`schema_json::prechunked_default_joins_chunks_into_one_call`)

## 10.1 Simple engine

```
Algorithm 1  SimpleExtract
Require: text T (UTF-8, non-empty after trim), config C
Ensure:  returns ExtractionResponse whose graph is the per-chunk graphs folded
         per C.spec.merge_duplicates / merge_strategy / coref; every record
         carries citations computed from chunk offsets
 1: if T.trim() is empty then error "No input text provided"
 2: if T.chars().count() < C.min_segment_size ∧ ¬C.quiet then warn (non-fatal)
 3: if input is pre-chunked then segments ← given chunks
    else if T.chars().count() > C.segment_size
         then segments ← Segment(T, C.chunker, C.segment_size, C.overlap)
                  filtered by min_segment_size on non-first segments
    else segments ← [whole text as one segment with offsets [0, |T|])]
 4: for each segment derive lines ← LineIndex(T).line_range(seg.start, seg.end)
    (pre-chunked: lines come from chunk metadata)
 5: graphs ← segments mapped concurrently (max C.max_concurrency in flight,
    order preserved) through PerChunkSession (Algorithm 2), each stamped with
    Citation(C.source_doc, seg.lines)
 6: kg ← Fold(graphs) per Algorithm 4
 7: return ExtractionResponse(kg, parsed_results)
```

```
Algorithm 2  PerChunkSession
Require: backend session S (native or Replay), chunk text, gleanings G, relation-gleanings R
Ensure:  returns (parsed_results, chunk_graph) with entities/triples collected
         across the extraction + entity-gleaning turns, plus rescued edges
 1: entities ← ∅; triples ← ∅; results ← []
 2: for i in 0..G:
      prompt ← if i=0 extraction_prompt else continue_prompt
      out ← S.send(prompt); if empty/error: stop
      parsed ← ParseDelimiter(out)            ▷ Algorithm 3
      add parsed.entities (first-wins) and parsed.triples
      if i>0 ∧ added nothing: stop            ▷ early stop on idle gleaning
 3: for r in 0..R:
      orphans ← entities with no incident triple
      if orphans empty: stop
      out ← S.send(relation_gleaning_prompt(orphans))
      rescued ← ParseRelationsAgainst(out, known entities)   ▷ drops unknown endpoints
      keep only triples whose (s,p,o) tuple is new
      if none new: stop
      add rescued (tagged metadata "relation_gleaned"=true)
 4: return (results, KnowledgeGraph(entities, triples))
```

### Simple delimiter grammar (ABNF)

The Simple engine drives the model to emit records of this shape; the parser
accepts them. `TUPLE_DELIMITER = "<|>"`; `RECORD_DELIMITER = "##"`.

```abnf
output       = *( record [record-delim] )
record-delim = "##"
record       = entity-rec / relation-rec
entity-rec   = "(" [ quote ] "entity" [ quote ] tuple-delim
                name tuple-delim type tuple-delim description
                [ tuple-delim attributes ] ")"
relation-rec = "(" [ quote ] "relationship" [ quote ] tuple-delim
                source tuple-delim target tuple-delim predicate
                [ tuple-delim description [ tuple-delim strength ] ] ")"
tuple-delim  = "<|>"
quote        = %x22 / %x27        ; " or '
```

Records are classified by their **leading token** (`entity` / `relationship`),
not by a whole-line substring scan, so a relationship whose description
contains the word "entity" is not misrouted. [T]
(`simple::relationship_with_entity_in_text_is_not_dropped`)

```
Algorithm 3  ParseDelimiter
Require: output O (delimiter records)
Ensure:  returns entities + triples; unknown-endpoint relations dropped
 1: items ← split O on RECORD_DELIMITER, then on entity/relation record starts
 2: for each item classified by leading token:
      entity → Entity(entity_id(clean_name), clean_type, clean_description, attrs)
               keyed by lowercased clean_name
      relationship → infer PredicateType from the predicate token;
                     drop if either endpoint name is unknown
 3: for each surviving relation:
      if predicate is *_BY and subject is an actor type and object is not:
         swap subject/object                  ▷ passive-voice auto-reverse
      triple.confidence ← strength clamped to [0,1] (default 0.8)
      metadata ← {description, source_name, target_name}
 4: return entities, triples
```

Predicate inference: a direct `PredicateType::resolve(predicate)`; if that
yields `RELATED_TO` but the predicate is not literally "related_to", a
keyword heuristic (`use/utilize/employ/apply`→USES, `part of/belong/...`→PART_OF,
`located/based in`→LOCATED_IN, `work/employed/affiliated`→WORKS_FOR,
`own/possess/has`→HAS_PROPERTY, else RELATED_TO) is applied. The keyword
table is **implementation-defined**; the contract is that a token that does
not resolve directly still produces a deterministic predicate.
[T] (`simple::relationship_type_token_maps_to_enum_predicate`)

### 10.1.x Citation granularity

- Simple / Agentic: each record cites the chunk/slice range containing the
  mention. Relation-gleaning records that look at the whole session cite the
  full range.
- SchemaJson / ToolCall (single-shot): every record cites the whole document
  `[1, total_lines]`.

## 10.2 SchemaJson engine

```
Algorithm 5  SchemaJsonExtract
Require: text T (non-empty), config C
Ensure:  returns graph from one model call; Fixed mode hard-drops out-of-schema
         records and reports them; citations = whole document
 1: validate input; if mode ∈ {Fixed,Evolving} ∧ schema empty ∧ no template:
      error "schema mode X requires a non-empty schema"
 2: prompt ← template-rendered OR (system prompt shaped by mode + "Text:\n{T}")
 3: resp ← backend.complete(prompt); on error return empty graph
 4: data ← extract_json(resp)  (```json fence, ``` fence, else whole string)
 5: if mode=Fixed ∧ schema non-empty: (data, drops) ← EnforceFixed(data) ▷ Alg 6
 6: kg ← BuildGraph(data): entities keyed by lowercased name (merge_strategy
      applies to within-response duplicates); relations resolved
      case-insensitively; dangling dropped; entity-type fallback = PHYSICAL_OBJECT
 7: stamp whole-document citation; record metadata (mode, schema_mode, drops…)
 8: return ExtractionResponse(kg)
```

```
Algorithm 6  EnforceFixed
Require: parsed data D, seed schema S (nodes, relations)
Ensure:  prunes D so only in-schema records survive; returns pruned data + drops
 1: an entity survives iff its resolved type ∈ S.nodes (empty S.nodes ⇒ keep all)
 2: a relation survives iff its predicate ∈ S.relations
    AND both endpoint entities survived
 3: dropped entity types and dropped predicate types are recorded
```

[T] (`schema_json::fixed_mode_drops_out_of_schema_records`,
`schema_json::fixed_mode_keeps_in_schema_records`,
`schema_json::open_mode_does_not_enforce_schema`)

Note: SchemaJson's entity-type fallback is **PHYSICAL_OBJECT** (its own quirk,
distinct from `from_loose`'s `OTHER`); relations fall back to `RELATED_TO`.
This asymmetry is observable and part of the contract.

## 10.3 ToolCall engine

Six tools (`add_entity`, `add_relation`, `add_attribute`,
`propose_schema_type`, `list_entities` *(multi-round only)*, `finish`). The
`type` / `predicate` tool args are JSON-Schema `enum`-constrained **only** in
Fixed mode with a non-empty schema; Open/Evolving leave them free-form.

```
Algorithm 7  ToolCallExtract
Require: text T, config C, backend B with supports_tools() = true
Ensure:  graph from accumulated tool calls; Fixed = enum-constrained;
         Evolving records proposed types; relations reference entity names
 1: validate input; enforce non-empty-schema gate for Fixed/Evolving
 2: if ¬B.supports_tools(): error
 3: rounds ← 0..max_rounds:
      resp ← B.complete_with_tools(messages, tools)
      if resp.tool_calls empty: stop
      echo assistant tool_calls; for each call apply → accumulator
      if finished: stop
 4: kg ← BuildGraph(accumulator): entities by lowercased name; relations
      resolved by name (dangling dropped); strength clamped [0,1] (default 0.8);
      attributes applied last (survive triple re-inserts)
 5: if merge_duplicates: kg ← dedup per merge_strategy/coref
 6: stamp whole-document citation; record mode/schema_mode/new_schema_types
```

| Tool call | Effect | Required args |
|-----------|--------|---------------|
| `add_entity` | record entity | `name`, `type` |
| `add_relation` | record relation (drops if endpoint unknown) | `source`, `predicate`, `target` |
| `add_attribute` | set metadata key on a known entity | `entity`, `key`, `value` |
| `propose_schema_type` | record proposed type (Evolving) | `kind ∈ {node,relation,attribute}`, `name` |
| `list_entities` | return recorded names (multi-round only) | — |
| `finish` | stop the loop | — |

[T] (`toolcall::single_round_collects_tool_calls`,
`toolcall::relation_strength_is_clamped`,
`toolcall::dangling_relation_is_dropped`,
`toolcall::evolving_mode_records_proposed_types`)

## 10.4 Agentic engine (experimental)

Drives the **whole document through one continuous multi-turn SDK session** in
a read-only tool sandbox (Read/Grep/Glob allowed; Write/Edit/Bash denied). The
text is split into slices; each slice is one turn, so slice N reuses entity
names seen in slices 1..N-1 (extraction-time coreference). After all slices a
whole-graph relation-gleaning turn connects orphan entities across slices.

Schema modes map onto the session:

| Mode | Behaviour |
|------|-----------|
| `open` | schema is hints; model extracts freely |
| `fixed` | per-slice validation loop: out-of-schema records dropped, next turn carries a correction; a fully-out-of-schema slice is redone once |
| `evolving` | seed types preferred, nothing dropped; types used outside the seed recorded as `new_schema_types` |

The Agentic engine is strictly sequential (no chunk concurrency). Its exact
slice-count, sandbox tool list, and correction-prompt wording are
**implementation-defined**; the contract is the single-session, cross-slice
coreference + per-slice Fixed validation + whole-graph gleaning shape above.

## 10.5 Graph fold (merge)

```
Algorithm 4  Fold
Require: list of chunk graphs, dedup flag, strategy, coref
Ensure:  one graph; label-duplicates combined; triples deduped by to_tuple;
         citations unioned
 1: if ¬dedup: left-to-right KnowledgeGraph.merge
 2: else if strategy=llm: left-to-right merge_with_llm (backend synthesises
      differing descriptions; falls back to field-union)
 3: else: left-to-right merge_with_coref(strategy, coref):
      recognize duplicates (exact / fuzzy per 8.7)
      combine per strategy (8.6), keeping existing.id
      remap triple endpoints to canonical ids; drop duplicate triples;
      union citations of merged duplicates and of duplicate triples
```

Cross-operation invariants:

- **Id stability:** merging never changes the canonical (existing) entity id;
  a colliding different-label entity that reuses an id gets a fresh `[N+1]` id
  whose stored `Entity.id` equals the key. [T]
  (`merger::dedup_collision_rewrites_entity_id_to_new_key`)
- **Triple-endpoint consistency:** a merged triple's endpoints bind to the
  canonical (merged) entity snapshot, never a stale lower-confidence one.
  [T] (`graph::merge_does_not_clobber_higher_confidence_entity_via_triple_endpoint`)
- **Triple dedup within source:** identical triples within a single folded
  graph collapse to one. [T]
  (`graph::merge_dedups_identical_triples_within_other`)

## 11. Validation / Diagnostics / Error model

There is no formal error-code schema. Diagnostics are human-readable. The
**stable shapes** are:

| Trigger | Outcome | Severity |
|---------|---------|----------|
| empty/whitespace input | error (extract aborts) | error |
| `Fixed`/`Evolving` with empty schema (and no template) | error naming the mode + fix hint | error |
| ToolCall on a non-tool backend | error "backend does not support tool calling" | error |
| model call failure (SchemaJson) | empty graph returned (Ok) | degenerate |
| session/turn failure (Simple) | warn to stderr; stop that chunk | warning |
| input smaller than `min_segment_size` | warn to stderr (non-fatal) unless `quiet` | warning |
| Fixed dropped records | counted in `schema_dropped_records` / `schema_dropped_types`; summarized to stderr | info |
| type aliasing/fallback | `type_normalization` in response metadata; one-line stderr summary | info |

Diagnostic strings are **not** a stable contract beyond the category + outcome
above; callers MUST NOT match on exact wording.

## Retry / timeout / cancellation

No engine implements retry, timeout, or cancellation around model calls.
A failed call surfaces (error or, for SchemaJson, an empty graph). This is a
deliberate absence, not a gap to be filled.

## KgStore load-modify-save lifecycle (state table)

Each KgStore mutation is one load-modify-save cycle on `<output>/<path>.json`,
serialized by an internal lock so concurrent tool calls cannot clobber one
file.

| (state of path, event) | (state', action, output) |
|------------------------|--------------------------|
| (no file, add_entity) | (1 entity, create file+dirs, ok) |
| (file, add_entity same name) | (updated entity, merge type/desc/attrs+citation, ok) |
| (file, add_relation missing endpoint) | (unchanged, error naming missing entity + known list) |
| (file, add_relation both present, new tuple) | (+triple, ok "stored") |
| (file, add_relation both present, existing tuple) | (citation unioned, ok "already present") |
| (file, add_attribute unknown entity) | (unchanged, error + known list) |
| (any, propose_schema_type when mode ≠ Evolving) | (unchanged, error) |
| (no file, query_graph) | (unchanged, error "call add_entity first") |
