# P0 Protocol Contract Lock

This contract freezes the cross-package behavior required before further KG
algorithm work. It covers the Rust producer (`kg-extract`), the shared Rust and
Python protocol projections, and the Python GraphRAG consumer.

Contract frozen at 2026-07-15T08:44:42-07:00. No modifications without a
recorded rationale. New assertions may be appended.

Amendment log (append-only):

- `VAL-KG-PROTOCOL-007` was added after producer-only bbox tests exposed that
  `kg-extract` discarded page/bbox on its pre-chunked input boundary. Its
  acceptance path is explicitly `SimpleExtractor`, the default chunk-aware
  CLI engine; single-shot engines cannot truthfully assign a joined result to
  one input chunk.
- `VAL-KG-PROTOCOL-008` was added after review found that the first golden
  relation contradicted both its passive predicate and its evidence quote.

## Validation Contract

- [x] **VAL-KG-PROTOCOL-001 (BLOCKING)**: `kg-extract` builds and runs its library tests against one reproducibly pinned revision of each Git protocol dependency.
  - **Verify**: Run `cargo test --lib --locked` in `kg-extract` from a clean checkout.
  - **Evidence**: Exit code 0; Cargo does not resolve a different Git commit; no `SourceRange` struct-literal compatibility error.
  - **Expected**: The current `bbox` addition is handled without removing bbox from the shared protocol.

- [x] **VAL-KG-PROTOCOL-002 (BLOCKING)**: A valid protocol relation with no incoming relation ID can be consumed by `kg-graphrag` and receives a stable non-empty internal ID.
  - **Verify**: Run the Python protocol bridge test using `KgRelation(id=None, ...)` twice with identical content.
  - **Evidence**: Both conversions succeed and produce the same non-empty string ID.
  - **Expected**: The bridge derives an ID from canonical relation content rather than relying on a random UUID.

- [x] **VAL-KG-PROTOCOL-003 (BLOCKING)**: GraphRAG bridge metadata never overwrites or deletes a user property that happens to use the bridge's reserved key.
  - **Verify**: Round-trip an entity and a relation whose properties contain the reserved key.
  - **Evidence**: Either the original value is preserved exactly or conversion rejects the collision explicitly; silent overwrite is forbidden.
  - **Expected**: Conversion rejects the collision with a clear validation error.

- [x] **VAL-KG-PROTOCOL-004 (BLOCKING)**: Multimodal bbox provenance uses the shared `core-types-rs::BBox` in `SourceRange`, with no duplicate `mm_bbox` metadata representation.
  - **Verify**: Convert a multimodal item with page and bbox into a protocol `Chunk` and serialize it.
  - **Evidence**: JSON contains `range.bbox`; `metadata.mm_bbox` is absent; coordinates round-trip unchanged.
  - **Expected**: `kg-multimodal` no longer defines a parallel bbox type.

- [x] **VAL-KG-PROTOCOL-005 (BLOCKING)**: Rust and Python accept and re-emit the same canonical KG protocol fixture without semantic drift.
  - **Verify**: Run the Rust golden test and the Python golden-corpus test over the same fixture containing entity, relation, evidence, page, and bbox.
  - **Evidence**: Canonical JSON values are identical, including optional-field omission behavior.
  - **Expected**: Both projections emit `kg.protocol.v1` and preserve all fixture fields.

- [x] **VAL-KG-PROTOCOL-006**: Invalid confidence values and dangling relation endpoints are reported by an explicit semantic validator without weakening lossless wire parsing.
  - **Verify**: Parse invalid documents successfully as wire DTOs, then call semantic validation.
  - **Evidence**: Validation reports confidence and endpoint violations with stable machine-readable codes.
  - **Expected**: Wire compatibility and domain validity remain separate concerns.

- [x] **VAL-KG-PROTOCOL-007 (BLOCKING)**: Pre-chunked multimodal provenance survives `kg-extract` and appears on emitted entity and relation evidence.
  - **Verify**: Feed one protocol `Chunk` carrying page and bbox through `SimpleExtractor::extract_prechunked`, then convert the resulting graph to `KgDocument`.
  - **Evidence**: Every extracted record stamped from that chunk carries the same full `SourceRange` as first-class evidence; `properties.citations` is absent, so provenance has one protocol representation.
  - **Expected**: The default chunk-aware extraction path preserves the full source range instead of retaining only line coordinates.

- [x] **VAL-KG-PROTOCOL-008**: The canonical golden relation uses the declared predicate direction.
  - **Verify**: Parse the shared golden fixture and inspect its `DEVELOPED_BY` relation.
  - **Evidence**: Subject is the developed product (`entity_gpt4`) and object is the developer (`entity_openai`).
  - **Expected**: The fixture agrees with its quote, `GPT-4 DEVELOPED_BY OpenAI`.
