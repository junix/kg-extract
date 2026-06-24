# 05 — Conformance

## 13. Examples

### Happy path — Simple delimiter extraction

Input text `"OpenAI developed GPT-4."`; mock backend returns:

```
(entity<|>OpenAI<|>organization<|>An AI research lab.<|>)##
(entity<|>GPT-4<|>technology<|>A large language model.<|>)##
(relationship<|>OpenAI<|>GPT-4<|>uses<|>OpenAI develops GPT-4.<|>0.9)##
```

Observed result: 2 entities, 1 triple, predicate `USES`. Second gleaning turn
empty → early stop. (source: `simple::extracts_entities_and_relationship`)

### Boundary — passive-voice auto-reverse

`developed_by` relation between an actor (`organization`) and a non-actor
(`product`): the Simple engine reverses subject/object so the actor becomes
the object, yielding `Aurora Portal --developed_by--> Helio Systems`. (source:
`simple::passive_by_relation_reverses_actor_to_product_direction`)

### Boundary — Fixed schema hard-drop

Schema `{nodes:[ORGANIZATION], relations:[DEVELOPED_BY]}`; model leaks a
`TECHNOLOGY` entity and a `USES` relation. Fixed mode drops both (the
`DEVELOPED_BY` relation also drops because its `GPT-4` endpoint was dropped);
`schema_dropped_records = 3`, dropped types include `TECHNOLOGY` and `USES`.
(source: `schema_json::fixed_mode_drops_out_of_schema_records`)

### Failure — degenerate schema cell

`Fixed` mode on an empty schema (no template) → `extract` returns an error,
not a silent "use only types from []". (source:
`schema_json::fixed_mode_without_schema_errors`)

### Failure — MCP missing endpoint

`add_relation` referencing an entity not yet stored → tool error naming the
missing endpoint and listing known entity labels, never a silent stub.
(source: `mcp.rs` `add_relation_with_citation`)

## 14. Definition of Done

Behaviors are grouped into clusters. `[T]` = covered by an existing test
(cited); `[U]` = currently uncovered, needs an acceptance test.

### A. Identity & determinism

- A.1 `entity_id(name)` is `entity_` + the first 8 hex chars of `md5(name)`
     over the raw name bytes; equal for equal bytes. `[T]`
- A.2 Entity ids are identical across two independent runs of the same engine
     on the same input. `[T]`
- A.3 Engine ids and MCP-store ids use the same scheme (interchangeable
     files). `[T]` (MCP `entity_id` shared)

### B. Graph invariants

- B.1 The entity map is insertion-ordered; emit/merge preserve order. `[T]`
- B.2 `add_triple` re-inserting an endpoint never erases the stored entity's
     citations. `[T]`
- B.3 Merging a duplicate entity never changes the canonical (existing) id;
     a colliding different-label entity gets a fresh id whose value equals
     its key. `[T]`
- B.4 A merged triple's endpoints bind to the canonical (merged) entity, not
     a stale snapshot. `[T]`
- B.5 Identical triples within a folded graph collapse to one. `[T]`

### C. Schema modes

- C.1 `Open` is the default and needs no schema. `[T]`
- C.2 `Fixed`/`Evolving` with an empty schema (no template) is an error. `[T]`
- C.3 `Fixed` hard-drops out-of-schema entities and relations (and relations
     whose endpoint was dropped), recording counts + types. `[T]`
- C.4 `Fixed` keeps in-schema records and reports `schema_dropped_records=0`. `[T]`
- C.5 `Open` does not drop or annotate. `[T]`
- C.6 `Evolving` records proposed types under `new_schema_types`. `[T]`
- C.7 A template-driven Fixed run with an empty schema succeeds (template
     ignores the schema requirement). `[T]`

### D. Type normalization

- D.1 `resolve` distinguishes Exact / Aliased / Fallback and the graph's
     `type_normalization` report records aliased/fallback tokens (absent when
     all exact). `[T]`
- D.2 `output_type` exposes the raw token when present, else the enum value. `[T]`

### E. Merge & coreference

- E.1 `keep-existing` keeps the first occurrence; `keep-incoming` replaces
     with the later (id preserved); `field-union` takes max confidence, the
     longer description, specific type over `Other`, unions metadata. `[T]`
- E.2 `llm` synthesises a merged description when both differ & non-empty,
     else field-union. `[T]`
- E.3 Citations from both sides survive every strategy. `[T]`
- E.4 `coref=off` keeps surface variants separate; `coref=fuzzy` collapses
     normalized variants and near-typo (length≥6, type-compatible) and remaps
     triples onto the canonical id. `[T]`
- E.5 Fuzzy never fuses labels shorter than 6 chars. `[T]`

### F. Simple engine specifics

- F.1 Records are classified by leading token, not line substring; a relation
     mentioning "entity" survives. `[T]`
- F.2 A non-resolving predicate token yields a deterministic predicate via
     the keyword fallback. `[T]`
- F.3 Passive `*_BY` actor→non-actor misdirection is auto-reversed. `[T]`
- F.4 Relation-gleaning rescues orphan entities (tagged `relation_gleaned`)
     and is off by default. `[T]`
- F.5 An idle entity-gleaning turn triggers early stop. `[T]`
- F.6 An unmatched `]` in an attribute string does not drop later attributes. `[T]`

### G. Pre-chunked input

- G.1 The chunking engines consume given chunks as-is (no re-chunking); empty
     input errors. `[T]`
- G.2 The single-shot engines join chunks with `\n\n` into one call. `[T]`
- G.3 JSON array, `{"chunks":[...]}` wrapper, and JSONL (with truncation
     trailer skipped) are all accepted; offsets synthesize cumulatively when
     absent; missing `text` and empty input are errors. `[T]`

### H. Provenance

- H.1 Line ranges derive from chunk char offsets (LineIndex); empty slice
     cites its start line. `[T]`
- H.2 Simple/Agentic cite the chunk/slice range; single-shot cite the whole
     document. `[T]`
- H.3 A record in several chunks accumulates multiple citations; merging
     unions them; duplicates are skipped by value. `[T]`
- H.4 Pre-chunked provenance comes from chunk metadata; chunks without line
     metadata yield unstamped records. `[T]`

### I. ToolCall engine

- I.1 Single-round collects every tool call into a graph. `[T]`
- I.2 Relation `strength` clamps to [0,1]; default 0.8. `[T]`
- I.3 Dangling relations are dropped; `list_entities` appears only in
     multi-round. `[T]`
- I.4 Fixed mode enum-constrains type/predicate args; a non-tool backend
     errors. `[T]`

### J. Output formats

- J.1 `node-link` uses `source`/`target` referencing node `id`s; no RDF keys
     leak. `[T]`
- J.2 `kg-protocol` lifts citations into first-class evidence ranges and
     carries `normalized_*_type` properties. `[T]`

### K. MCP KgStore

- K.1 `path` resolution rejects absolute and `..`; cannot escape `<output>`. `[U]`
- K.2 `add_relation` requires both endpoints present (else actionable error
     with known labels). `[U]`
- K.3 Duplicate `(s,p,o)` relations merge citations, not duplicate the record. `[U]`
- K.4 Source-citation fields validate (relative, exists, fits line count). `[U]`
- K.5 `Fixed` rejects out-of-schema types; `Evolving` accepts proposed types
     after `propose_schema_type`. `[U]`

> Note on `[U]`: the KgStore behaviours are exercised by `mcp_test.rs`
> (in-tree, not read in detail for this pass). They are marked `[U]` here
> pending an explicit Test↔DoD mapping pass; the contract statements above
> are derived from the implementation and the README and are normative.
