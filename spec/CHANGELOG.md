# CHANGELOG

Append-only session ledger for the kg-extract specification. Each non-STABLE
mode appends exactly one entry.

## 2026-06-24T00:19 — CREATED

Full normative spec authored for kg-extract (no prior `spec/`).

- Files: 00, 01, 02, 03, 04, 05, 06
- Code basis: a0b4de0
- Seed scope: all four engines (Simple/SchemaJson/ToolCall/Agentic), the three
  schema modes, merge/coref/citation contracts, CLI + 7 output formats, and
  the MCP KgStore. Evidence drawn from tests + code + README.
- Feature matrix: 37 rows; 26 done / 11 partial / 0 missing. Partial clusters:
  MCP server (X-02..X-05), output formats O-02/O-05, backends B-01..B-03 —
  recommended targets for a follow-up EXTENDED pass.
- Notable: `[U]` DoD items in chapter 05 cluster K (MCP KgStore) await an
  explicit Test↔DoD mapping over `mcp_test.rs`; the contract statements are
  normative pending that tag promotion. Type-vocabulary size, alias tables,
  corporate-suffix/article lists, Agentic slice-count, and exact prompt
  wording recorded as implementation-defined, not contractual constants.

## 2026-07-15T09:21 — UPDATED

Aligned the specification with the cross-package provenance contract lock.

- Files: 00, 01, 02, 03, 04, 05, 06, 07, CHANGELOG.
- Citation wire contract: legacy line-only entries remain `{doc,lines}`;
  citations carrying page/bbox use `{doc,range:SourceRange}` and preserve the
  complete supplied range.
- Protocol projection: a fully recognized array in either citation shape
  becomes first-class `Evidence`; its internal key is removed to avoid a
  duplicate representation. Foreign, malformed, or mixed `citations` values
  stay in protocol properties rather than being partially promoted or lost.
- Engine boundary: Simple/Agentic retain per-chunk provenance; single-shot
  SchemaJson/ToolCall join chunks and provide document-level provenance only.
  Conflicting non-empty pre-chunked `source_file` values are rejected.
- Rust API compatibility: public `Segment.lines` became `Segment.range`, and
  `Citation.{start_line,end_line}` became `Citation.range`. Both break direct
  struct literals/field access; `Citation` is now `PartialEq` but not `Eq`
  because bbox coordinates are floating-point. `Citation::new(doc,start,end)`
  remains compatible for valid line ranges. Pre-chunked JSON and legacy
  line-citation JSON remain wire-compatible through optional protocol `range`
  fields and the unchanged `{doc,lines}` form.
- Build reproducibility convention: protocol-facing Git dependencies require
  exact revisions, `Cargo.lock` is tracked, and local path patches remain
  transient rather than committed manifest/lock state.

## 2026-08-08T12:43 — UPDATED

Closed the kg-multimodal → kg-extract chunk-payload contract gap.

- Files: 02, 04, 07, CHANGELOG.
- Pre-chunked input (§9.5): the optional `title` and `metadata` (JSON object)
  of a protocol `Chunk` are no longer dropped at the input boundary. The
  chunk-aware engines (Simple/Agentic) stamp them onto every record extracted
  from that chunk as the metadata keys `chunk_title` / `chunk_metadata`; the
  protocol projection passes them through to entity/relation properties.
  Single-shot engines (SchemaJson/ToolCall) join chunks and keep
  document-level provenance only, unchanged. A non-object `metadata` is a
  parse error.
- Mixed-source error made actionable: conflicting non-empty `source_file`
  values now report the offending chunk index, both files, and the remedy
  (split the input by `source_file`, run once per document).
- Rust API compatibility: public `Segment` gained `title: Option<String>` and
  `metadata: BTreeMap<String, Value>` — a source-breaking change for struct
  literals; the pre-chunked wire format only grew optional keys and stays
  compatible.
- New contract assertion: VAL-KG-PROTOCOL-009 (07), with amendment-log entry.
- New regression tests: `chunking::prechunked_preserves_title_and_metadata`,
  `chunking::prechunked_rejects_non_object_metadata`,
  `citation::tests::stamp_chunk_metadata_attaches_title_and_metadata_to_every_record`,
  `citation::tests::stamp_chunk_metadata_is_noop_without_payload`,
  `simple::tests::prechunked_title_and_metadata_reach_protocol_properties`;
  `chunking::prechunked_rejects_conflicting_source_files` extended for the
  actionable message.

## 2026-08-08T16:20 — UPDATED

Fixed the triple dedup key to use the normalized predicate value.

- Files: 02.
- `to_tuple`'s middle component changed from `predicate.output_type` (the raw
  model-emitted surface token) to `predicate.predicate_type.value` (the
  normalized canonical value). Cross-chunk surface variants such as
  `"uses"`/`"USES"` or `"founded by"`/`"FOUNDED_BY"` previously escaped dedup
  in every path that keys on `to_tuple` (`KnowledgeGraph::merge`,
  `merger::remap_triples`, Simple/Agentic intra-pass dedup, MCP store) and
  produced duplicate edges; they now collapse to one.
- New regression tests: `graph::to_tuple_uses_normalized_predicate_type_not_raw_surface_form`,
  `graph::merge_dedups_surface_variant_predicates_across_chunks`,
  `graph::merge_keeps_distinct_normalized_predicates`.

## 2026-08-08T20:18 — UPDATED

Three enhancements: fuzzy-coref token-set channel, multiplicity-weighted
community edges, and a CLI community output.

- Files: 02, 04, 05, 06, CHANGELOG.
- Coref (§8.7): `CorefMode::Fuzzy` gains a **token-set channel** alongside
  edit distance. Over normalized token sets, a pair is coreferent when the
  smaller set is fully contained in the larger with ≥ 2 tokens (score 1.0;
  single-token containment rejected as too generic), or when Jaccard
  |∩|/|∪| ≥ 0.6. No length gate (unlike the ≥6 edit-distance gate); the
  type-compatibility gate and earliest-inserted tie-break are unchanged. No
  new `CorefMode` variant — the channel extends `Fuzzy`, keeping `--coref`
  and the spec's two-mode contract intact. Default (`off`) behavior
  unchanged.
- Community mapping (new §8.11): `to_community_graph` no longer dedups
  edges — every triple becomes one weight-1.0 undirected edge and
  `kg-community` sums parallel edges, so triple multiplicity between two
  entities acts as community-strength signal (previously flattened by
  `Graph::from_edges`). Self-loops / unregistered endpoints still skipped.
- CLI (§9.3, §9.4): new output format `-o communities` emitting
  `{num_communities, communities: {"0": [entity_id, …], …}}` via
  label-propagation detection (feature `community`). The value parses without
  the feature but the run then fails with an actionable error (mirroring
  `--backend llms` without `llms-backend`). `just build` / `just lint`
  feature sets now include `community`.
- Conformance: new E.6 (token-set coref), J.3 (communities output), and
  cluster L (community adapter: L.1 multiplicity-as-weight, L.2
  partition-changing weights), all `[T]`.
- Feature matrix: M-02 row widened to both coref channels; new O-08
  (communities output) and D-01 (community adapter) rows; done count 26 → 28.
- New regression tests: `merger::token_set_similarity_subset_and_jaccard`,
  `merger::coref_fuzzy_token_set_merges_subset_and_jaccard_surfaces`,
  `merger::coref_fuzzy_token_set_respects_type_gate_and_short_names`,
  `merger::coref_fuzzy_token_set_is_deterministic_earliest_wins`,
  `merger::coref_fuzzy_merges_abbreviation_with_suffixed_form`,
  `community::multiplicity_produces_parallel_weighted_edges`,
  `community::multiplicity_weights_change_the_partition`,
  `community::communities_json_groups_members_by_community`,
  bin `print_response_communities_returns_ok_with_feature` /
  `print_response_communities_bails_without_feature` /
  `communities_output_format_parses_from_cli_and_config`.

## 2026-08-08T20:43 — UPDATED

kg-community bumped to 9da8120: engine quality scores and hierarchical
Leiden surface in the CLI.

- Files: 02, 04, 05, 06, CHANGELOG.
- Dependency: `kg-community` git pin moved 9f12ba1 → 9da8120 (tracked
  `Cargo.lock`, exact-rev convention unchanged). New pass-through feature
  `community-leiden = ["community", "kg-community/leiden"]` pulls in
  `leiden-rs`; `just build` / `just lint` feature sets now include
  `community-leiden`.
- Community mapping (§8.11): `communities_json` now emits a `quality` field
  sourced from `Partition::quality` (`null` for label propagation, which
  reports no score). New `hierarchy_json` (feature `community-leiden`)
  renders hierarchical Leiden as `{detector, num_levels, levels: [{level,
  quality, num_communities, communities}]}` — levels ordered coarse → fine
  (`level` 0 matches a flat Leiden run's grouping), per-level modularity,
  fixed-seed detection plus sorted member ids for run-to-run determinism.
- CLI (§9.3 grammar, §9.4 table): new output format
  `-o communities-hierarchy` (alias `hierarchy`). It parses without the
  feature but the run then fails with an actionable error naming
  `--features community-leiden`, mirroring the `communities` / `community`
  pattern.
- Conformance: J.3 widened with the `quality` field; new J.4
  (communities-hierarchy contract), all `[T]`.
- Feature matrix: O-08 row widened; new O-09 (communities-hierarchy output)
  and D-02 (hierarchy_json) rows; done count 28 → 30.
- New regression tests:
  `community::leiden_tests::hierarchy_json_layers_coarse_to_fine_with_quality`,
  `community::leiden_tests::hierarchy_json_is_deterministic_across_runs`,
  bin `print_response_communities_hierarchy_returns_ok_with_feature` /
  `print_response_communities_hierarchy_bails_without_feature` /
  `communities_hierarchy_output_format_parses_from_cli_and_config`;
  `community::communities_json_groups_members_by_community` now also pins
  `quality: null` for label propagation.

## 2026-08-08T21:02 — UPDATED

GraphRAG-style community summaries close the community-detection loop.

- Files: 02, 04, 05, 06, CHANGELOG.
- Community mapping (new §8.11.1): `CommunitySummary` + `summarize_partition`
  generate one `{name, summary}` report per community through the configured
  `LlmBackend`; `communities_json_with_summaries` and
  `hierarchy_json_with_summaries` (feature `community-leiden`, every level)
  render each community as `{"members": [ids…], "name": …, "summary": …}`
  instead of a bare id array. The prompt lists member entities
  (`label (type): description`) and sorted intra-community triples, and asks
  for a bare JSON object parsed with the shared JSON extraction. Token bounds
  are contractual constants: `SUMMARY_MAX_MEMBERS = 32`,
  `SUMMARY_MAX_TRIPLES = 24`, `SUMMARY_MAX_DESC_CHARS = 120`,
  `SUMMARY_MAX_TOKENS = 1024`; overflows are noted as `(+N more …)`.
  Communities are processed in ascending-label order (levels coarse → fine),
  so the backend-call sequence is deterministic. Failed/unparseable calls
  degrade that community to null `name`/`summary` with a stderr warning
  naming the community (§11 silent-degradation model), never a run failure.
  Without summaries the community JSON shape is unchanged.
- CLI (§9.3, §9.4): new presence flag `--community-summaries` (config key
  `community_summaries`), valid with `-o communities` /
  `-o communities-hierarchy`; other formats emit a note and ignore it. The
  extraction backend is reused (agentic builds one on demand via
  `make_backend`). Without `--features community` the print arm bails naming
  the feature, unchanged.
- Conformance: new J.5 (community summaries), all `[T]`.
- Feature matrix: new O-10 (community summaries output) and D-03
  (summarize_partition) rows; done count 30 → 32.
- New regression tests:
  `community::summary_tests::summaries_render_as_objects_with_name_and_summary`,
  `community::summary_tests::summary_prompts_and_output_are_deterministic`,
  `community::summary_tests::failing_backend_degrades_to_null_fields`,
  `community::summary_tests::unparseable_reply_degrades_to_null_fields`,
  `community::summary_tests::prompt_truncates_members_triples_and_descriptions`,
  `community::leiden_tests::hierarchy_summaries_cover_every_level_and_stay_deterministic`,
  bin `community_summaries_flag_parses_from_cli_and_config`,
  `print_response_communities_accepts_precomputed_summaries`,
  `print_response_communities_hierarchy_accepts_precomputed_summaries`.

## 2026-08-09T01:24 — UPDATED

Community summaries run with bounded concurrency; config.example.json
re-aligned with the real FileConfig surface.

- Files: 02, 04, 05, 06, CHANGELOG.
- Community summaries (§8.11.1): `summarize_partition`,
  `communities_json_with_summaries`, and `hierarchy_json_with_summaries` take
  a new `max_concurrency` parameter (Rust API source-breaking for direct
  callers). Per-community backend calls now run through `buffered` with up to
  `max_concurrency` in flight (clamped to ≥ 1) instead of strictly
  sequentially; hierarchy levels are still processed sequentially, coarse →
  fine. The determinism contract shifts from "deterministic call sequence" to
  **deterministic output**: results are keyed by community label and collected
  in issue (ascending-label) order, so completion order never leaks into the
  rendered JSON — a concurrent run is byte-identical to a sequential one given
  the same backend replies. Per-community degradation to null
  `name`/`summary` is unchanged; stderr warning order is now
  completion-ordered and not contractual.
- CLI (§9.3): `--max-concurrency` (config key `max_concurrency`, default 8)
  now bounds **both** Simple per-chunk extraction and
  `--community-summaries` calls; flag help and the flag table updated
  accordingly. No new flag or config key.
- Conformance: J.5 widened to the bounded-concurrency /
  deterministic-output contract; D-03 row widened, all `[T]`.
- config.example.json: dropped the historical `youtu_agent` / `community` /
  `toolcall_agent` keys (rejected by `FileConfig`'s `deny_unknown_fields`)
  and added the real `max_concurrency` key; the file now parses as
  `FileConfig`, pinned by a regression test.
- New regression tests:
  `community::summary_tests::concurrent_output_is_byte_identical_to_sequential`,
  `community::summary_tests::concurrency_cap_bounds_in_flight_calls`,
  `community::summary_tests::concurrent_partial_failure_degrades_only_that_community`,
  bin `file_config_parses_config_example_json`.

## 2026-08-09T01:44 — UPDATED

kg-vocab bumped to v2 (crate 0.2.0, vocabulary version `kg.vocab.v2`).

- Files: 02, CHANGELOG.
- Data model (§8.3 Predicate): the resolution paragraph now states the v2
  predicate parsing semantics owned by kg-vocab — longest-match-first
  substring matching (either direction, `_` word boundary required,
  declaration order breaks ties) and the <3-char no-fuzzy-match fallback —
  replacing the previous one-line "same precedence as EntityType" pointer.
  Upstream intentional changes relative to v1: `"in"` no longer aliases to
  LOCATED_IN (falls back to RELATED_TO), and `"used"` resolves to IS_USED_BY
  (longest match) instead of USED_IN. `PredicateType::inverse()` and the
  `ENTITY_GROUPS` / `PREDICATE_GROUPS` tables are noted as available through
  the existing re-export; no consumer in this crate yet (direction
  normalisation at merge time is a candidate, pending a spec decision).
- No conformance rows changed: the contract remains the resolution
  precedence plus the Exact/Aliased/Fallback audit, both unchanged.
- New regression tests: `types::predicate::tests::kg_vocab_v2_parse_semantics`,
  `types::predicate::tests::kg_vocab_v2_inverse_and_groups`.

## 2026-08-09T01:57 — UPDATED

Spec decision landed: opt-in **canonical direction** normalisation consumes
`PredicateType::inverse()` (kg-vocab v2) at the merge stage.

- Files: 00, 02, 03, 04, 05, 06, CHANGELOG.
- Data model (new §8.3.1): the triple dedup key cannot collapse direction
  variants (`(A, USES, B)` vs `(B, IS_USED_BY, A)`). The new
  `ExtractionSpec.canonical_direction` (default `false`) rewrites every
  triple to a canonical direction before dedup
  (`merger::normalize_direction`): a triple on the non-canonical member of an
  inverse pair is flipped — endpoints swapped, `predicate_type` →
  `inverse()` — so variants share one key. **Canonical-member rule: the
  variant declared first in `PredicateType` (vocab.json declaration order)**,
  chosen because declaration order is already the vocabulary's tie-breaker
  and needs no hand-maintained table; current canonical members:
  `IS_USED_BY`, `DERIVES_FROM`, `PART_OF`, `CONTRIBUTES_TO`, `SUCCEEDS`,
  `MEASURED_BY`. On a flip the stale `raw_type`/`label` surface token is
  cleared (it would otherwise display the inverted edge) and preserved in
  predicate metadata under `direction_normalized_from`; unpaired predicates
  untouched; pure per-triple deterministic transform. All four engines apply it at their merge/assembly stage (Simple:
  per chunk graph before the fold; ToolCall/SchemaJson: post-build,
  pre-dedup; Agentic: assembled triples). Community mapping (§8.11)
  consequently counts a merged direction-variant pair once.
- Interfaces (§9.3): new presence flag `--canonical-direction` + config key
  `canonical_direction` (CLI wins, else config; off by default).
- Runtime model (§10.3 ToolCall step, §10.5 Algorithm 4): normalisation step
  recorded before dedup.
- Conformance: new E.7, all `[T]`.
- Feature matrix: new M-05 row; done count 28 → 29.
- New regression tests:
  `merger::canonical_predicate_picks_first_declared_member_of_pair`,
  `merger::normalize_direction_flips_noncanonical_member_only`,
  `merger::direction_variants_dedup_to_one_edge_after_normalization`,
  `merger::direction_variants_survive_dedup_when_normalization_off`,
  `merger::normalize_direction_is_deterministic`,
  `toolcall::canonical_direction_flips_and_dedups_direction_variants`,
  `toolcall::direction_variants_stay_separate_by_default`,
  `community::canonical_direction_merges_direction_variant_multiplicity`,
  bin `canonical_direction_flag_parses_from_cli_and_config`.

## 2026-08-08T20:59 — UPDATED

kg-vocab bumped to v3 (crate 0.3.0, vocabulary version `kg.vocab.v3`).

- Files: 02, CHANGELOG.
- Data model (§8.3 Predicate): the resolution paragraph now states the v3
  semantics — a curated **disambiguation table** is consulted between exact
  match and the fuzzy substring scan, and an equal-length tie among the
  longest substring matches falls back to RELATED_TO instead of breaking by
  declaration order. Upstream intentional changes relative to v2: `"tested"`
  now resolves to TESTED_ON and `"validated"` to VALIDATED_ON (v2 returned
  TESTED_BY/VALIDATED_BY by declaration order); `"invented"`→INVENTED_BY and
  `"published"`→PUBLISHED_IN are pinned to their v2 results. Unchanged from
  v2: the <3-char no-fuzzy-match fallback, longest-match-first ordering, and
  the `_` word-boundary requirement.
- §8.3.1 rationale reworded: the canonical-member rule still pins the
  first-declared variant of each inverse pair, but no longer leans on
  declaration order being the vocabulary's parse tie-breaker (v3 removed
  that); the direction convention is unaffected.
- No conformance rows changed: the contract remains the resolution
  precedence plus the Exact/Aliased/Fallback audit.
- Guard tests renamed and extended:
  `types::predicate::tests::kg_vocab_v3_parse_semantics` (was
  `kg_vocab_v2_parse_semantics`; version assertion updated, disambiguation
  and tie-fallback assertions added),
  `types::predicate::tests::kg_vocab_v3_inverse_and_groups` (was
  `kg_vocab_v2_inverse_and_groups`).

## 2026-08-08T23:20 — UPDATED

Added the kg.provider/v1 provider-protocol surface for capability hubs.

- Files: 04, 05, 06, CHANGELOG.
- CLI (new §9.7): three subcommands — `describe --json` (one kg.provider/v1
  manifest: provider identity + six capabilities with `input_schema`,
  `side_effects`, `output`, and a `cli_spec` argv template following the acme
  CLIFlag convention), `available --json` (read-only env/PATH/feature probe,
  exit code always 0, `available ⇔ missing empty`), and `invoke
  <capability_id> --request (-|file)` (stdin/file JSON request → exactly one
  kg.execution/v1 envelope on stdout; artifacts carry `path`/`kind`/
  `checksum: sha256:<hex>`; `status: "error"` exits non-zero). The human
  `--describe` flag is unchanged and now points at the subcommands.
- Capabilities (ids frozen): `extract.entities_relations` (text/file →
  kg-document artifact; side effects network/data_egress),
  `detect.communities` / `detect.communities_hierarchy` (kg-document →
  communities JSON; local; features `community` / `community-leiden`),
  `summarize.communities` (kg-document → summarized communities;
  network/data_egress), `resolve.coref` / `resolve.canonical_direction`
  (kg-document → kg-document; local). The resolve pair runs the same passes
  as the extraction-time `coref` / `canonical_direction` options, exposed
  graph-in/graph-out so a hub can chain them after any graph producer.
  Uncompiled features keep the capability in `describe` but fail invoke with
  `backend_unavailable` (mirrors `-o communities` without the feature).
- Error model (`error.code`): `invalid_request`, `unknown_capability`,
  `backend_unavailable`, `extraction_failed`.
- New lib surface: `KnowledgeGraph::from_kg_document` (protocol.rs) imports a
  kg.protocol.v1 document back into the extractor-domain graph — tokens
  re-resolve through kg-vocab `resolve` with the original kept as `raw_type`,
  `normalized_*` properties are re-derived, ranged evidence becomes internal
  citations, dangling relations and range-less evidence are dropped and
  counted (`ImportReport`), and the counts surface as invoke diagnostics.
- Backend construction (`make_backend`, `parse_mock_tool_rounds`) moved from
  the CLI into `src/provider.rs` so invoke resolves backends identically to
  the CLI; the bin delegates. Behaviour unchanged.
- New dependency: `sha2` (artifact checksums).
- Conformance: new cluster M (M.1–M.8), all `[T]`.
- Feature matrix: new rows V-01..V-06 (all done); the done count is corrected
  to the true row arithmetic (37 pre-existing done, not 32) plus the six new
  rows → 43 done / 11 partial.
- New tests: 20 lib (`provider::tests::*`, `protocol::tests::from_kg_document_*`)
  + 5 bin (`provider_subcommands_parse`, cli_spec render-equivalence,
  `legacy_describe_points_at_provider_protocol`).
