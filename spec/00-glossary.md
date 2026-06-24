# Glossary

Canonical surface forms for the terms used across this specification. Each term
is referenced by its canonical form in the normative chapters; synonymous forms
in the source are listed and MUST be rewritten to the canonical form when
quoted.

| Term | Canonical surface form | Citation | Used in |
|------|------------------------|----------|---------|
| Entity | entity | test:schema_json::schema_based_extraction, code:types/entity.rs `Entity.to_dict` | 02, 03 |
| Triple | triple | test:graph::merge_dedups_identical_triples_within_other, code:types/graph.rs `Triple.to_dict` | 02, 03 |
| Predicate | predicate | test:schema_json::object_relationships_preserve_attributes_as_triple_metadata | 02, 03 |
| KnowledgeGraph | knowledge graph | test:simple::extracts_entities_and_relationships, code:types/graph.rs `KnowledgeGraph` | 02, 03 |
| SchemaMode | schema mode | test:schema_json::open_is_the_default_and_needs_no_schema, README §Schema modes | 02, 03, 04 |
| Schema | schema | test:schema_json::fixed_mode_drops_out_of_schema_records, code:types/schema.rs | 02, 03 |
| MergeStrategy | merge strategy | test:merger::field_union_combines_description_confidence_metadata_and_type | 02, 03 |
| CorefMode | coreference mode | test:merger::coref_fuzzy_merges_normalized_variant_and_remaps_triples | 02, 03 |
| ExtractionSpec | extraction spec | test:schema_json::one_spec_runs_through_both_engines, README §Spec vs execution | 02, 04 |
| ExtractionConfig | extraction config | code:types/config.rs `ExtractionConfig`, README §Spec vs execution | 02, 04 |
| Citation | citation | test:simple::prechunked_citations_use_chunk_metadata_lines, code:citation.rs `Citation` | 02, 03 |
| Segment | segment | test:simple::segments_long_text_and_merges_chunk_graphs, code:chunking.rs `Segment` | 02, 03 |
| Extractor | extractor | code:extractor/mod.rs `Extractor`, README | 03, 04 |
| Backend | backend | code:backend/mod.rs `LlmBackend`, README §Architecture | 03, 04 |
| GraphBuilder | graph builder | code:graph_build.rs `GraphBuilder` | 03 |
| KgStore | KG store | test:mcp (mcp_test.rs), code:mcp.rs `KgStore` | 04 |
