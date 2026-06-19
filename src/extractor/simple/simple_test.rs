use super::*;
use crate::backend::MockBackend;
use crate::graph_build::entity_id;

#[tokio::test]
async fn extracts_entities_and_relationship() {
    // 4th tuple field is the relationship type; it drives predicate inference.
    let resp_text = "(entity<|>OpenAI<|>organization<|>An AI research lab.<|>)##\
        (entity<|>GPT-4<|>technology<|>A large language model.<|>)##\
        (relationship<|>OpenAI<|>GPT-4<|>uses<|>OpenAI develops GPT-4.<|>0.9)##";
    // Second gleaning returns nothing new → early stop.
    let backend = Arc::new(MockBackend::new(vec![resp_text.into(), String::new()]));
    let ex = SimpleExtractor::new(backend);
    let out = ex.extract("OpenAI developed GPT-4.").await.unwrap();
    assert_eq!(out.num_entities(), 2);
    assert_eq!(out.num_triples(), 1);
    assert_eq!(
        out.knowledge_graph.triples[0].predicate.predicate_type,
        PredicateType::Uses
    );
}

#[tokio::test]
async fn relationship_type_token_maps_to_enum_predicate() {
    let resp_text = "(entity<|>OpenAI<|>organization<|>An AI research lab.<|>)##\
        (entity<|>GPT-4<|>technology<|>A large language model.<|>)##\
        (relationship<|>GPT-4<|>OpenAI<|>developed_by<|>GPT-4 was developed by OpenAI.<|>0.9)##";
    let backend = Arc::new(MockBackend::new(vec![resp_text.into(), String::new()]));
    let ex = SimpleExtractor::new(backend);
    let out = ex.extract("OpenAI developed GPT-4.").await.unwrap();
    assert_eq!(
        out.knowledge_graph.triples[0].predicate.predicate_type,
        PredicateType::DevelopedBy
    );
    assert_eq!(
        out.knowledge_graph.triples[0].metadata["description"],
        serde_json::json!("GPT-4 was developed by OpenAI.")
    );
}

#[tokio::test]
async fn passive_by_relation_reverses_actor_to_product_direction() {
    let resp_text = "(entity<|>Helio Systems<|>organization<|>A software company.<|>)##\
        (entity<|>Aurora Portal<|>product<|>A customer operations product.<|>)##\
        (relationship<|>Helio Systems<|>Aurora Portal<|>developed_by<|>Helio Systems developed Aurora Portal.<|>0.9)##";
    let backend = Arc::new(MockBackend::new(vec![resp_text.into(), String::new()]));
    let ex = SimpleExtractor::new(backend);
    let out = ex
        .extract("Aurora Portal is developed by Helio Systems.")
        .await
        .unwrap();
    let triple = &out.knowledge_graph.triples[0];
    assert_eq!(triple.subject.label, "Aurora Portal");
    assert_eq!(triple.object.label, "Helio Systems");
    assert_eq!(triple.predicate.predicate_type, PredicateType::DevelopedBy);
}

#[tokio::test]
async fn relation_gleaning_rescues_orphan_entities() {
    // First call extracts two entities but NO relationship -> both orphan.
    let extract = "(entity<|>OpenAI<|>organization<|>An AI research lab.<|>)##\
        (entity<|>GPT-4<|>technology<|>A large language model.<|>)##";
    // The rescue round emits a relationship only (no entity records). It must
    // still resolve against the already-known entities and become a triple.
    let rescue = "(relationship<|>OpenAI<|>GPT-4<|>uses<|>OpenAI develops GPT-4.<|>0.9)##";
    let backend = Arc::new(MockBackend::new(vec![extract.into(), rescue.into()]));
    let mut ex = SimpleExtractor::new(backend);
    ex.max_gleanings = 0; // exactly one extraction call, then one rescue round
    ex.max_relation_gleanings = 1;

    let out = ex.extract("OpenAI developed GPT-4.").await.unwrap();
    assert_eq!(out.num_entities(), 2);
    assert_eq!(
        out.num_triples(),
        1,
        "the orphan rescue round should add the edge"
    );
    let t = &out.knowledge_graph.triples[0];
    assert_eq!(t.predicate.predicate_type, PredicateType::Uses);
    assert_eq!(
        t.metadata.get("relation_gleaned"),
        Some(&serde_json::json!(true))
    );
}

#[tokio::test]
async fn relation_gleaning_off_by_default_keeps_orphans() {
    let extract = "(entity<|>OpenAI<|>organization<|>An AI research lab.<|>)##\
        (entity<|>GPT-4<|>technology<|>A large language model.<|>)##";
    let backend = Arc::new(MockBackend::new(vec![extract.into(), String::new()]));
    let mut ex = SimpleExtractor::new(backend);
    ex.max_gleanings = 0;
    // max_relation_gleanings defaults to 0 -> no rescue, orphans stay orphan.
    let out = ex.extract("OpenAI developed GPT-4.").await.unwrap();
    assert_eq!(out.num_entities(), 2);
    assert_eq!(out.num_triples(), 0);
}

#[tokio::test]
async fn relationship_with_entity_in_text_is_not_dropped() {
    // The relationship description mentions "entities"; the record must be
    // classified by its leading token, not a whole-line substring scan.
    let resp = "(entity<|>OpenAI<|>organization<|>An AI lab.<|>)##\
        (entity<|>GPT-4<|>technology<|>A model.<|>)##\
        (relationship<|>OpenAI<|>GPT-4<|>uses<|>OpenAI is the parent entity of GPT-4.<|>0.9)##";
    let backend = Arc::new(MockBackend::new(vec![resp.into(), String::new()]));
    let out = SimpleExtractor::new(backend)
        .extract("OpenAI and GPT-4.")
        .await
        .unwrap();
    assert_eq!(out.num_entities(), 2);
    assert_eq!(
        out.num_triples(),
        1,
        "relationship must survive even though its text contains the word 'entity'"
    );
}

#[test]
fn attributes_unmatched_bracket_does_not_drop_all() {
    // A stray `]` previously drove the depth counter negative, suppressing
    // every comma split so the whole attribute set was lost.
    let attrs = parse::parse_attributes_string("role: lead], team: platform");
    assert!(
        attrs.contains_key("role"),
        "first attribute must survive: {attrs:?}"
    );
    assert!(
        attrs.contains_key("team"),
        "later attribute must survive: {attrs:?}"
    );
}

#[test]
fn entity_id_is_deterministic_md5() {
    assert_eq!(entity_id("Openai"), entity_id("Openai"));
    assert!(entity_id("X").starts_with("entity_"));
}

#[tokio::test]
async fn segments_long_text_and_merges_chunk_graphs() {
    use crate::types::ChunkStrategy;
    // Two Char chunks, one LLM call each (gleaning off). Each chunk's canned
    // response carries a distinct entity; the merged graph must hold both —
    // proving segment → concurrent per-chunk extract → merge end to end.
    let r0 = "(entity<|>Alpha<|>organization<|>First chunk entity.<|>)##";
    let r1 = "(entity<|>Beta<|>organization<|>Second chunk entity.<|>)##";
    let backend = Arc::new(MockBackend::new(vec![r0.into(), r1.into()]));

    let mut cfg = SimpleExtractor::default_config();
    cfg.segment_size = 20;
    cfg.overlap = 0;
    cfg.min_segment_size = 1;
    cfg.chunker = ChunkStrategy::Char;
    cfg.max_concurrency = 2;
    let mut ex = SimpleExtractor::with_config(backend, cfg);
    ex.max_gleanings = 0; // one call per chunk → deterministic response mapping

    // 40 chars → exactly two 20-char Char chunks.
    let text = "a".repeat(20) + &"b".repeat(20);
    let out = ex.extract(&text).await.unwrap();

    let labels: Vec<&str> = out
        .knowledge_graph
        .entities
        .values()
        .map(|e| e.label.as_str())
        .collect();
    assert!(
        labels.contains(&"Alpha"),
        "chunk 0 entity must be present: {labels:?}"
    );
    assert!(
        labels.contains(&"Beta"),
        "chunk 1 entity must be present: {labels:?}"
    );
    assert_eq!(out.num_entities(), 2);
}

/// Pre-chunked input is consumed chunk-for-chunk: each given chunk gets its
/// own LLM call (no joining, no re-chunking). The two chunks together are
/// far below `segment_size`, so if the text were joined and re-segmented it
/// would collapse into ONE chunk and only the first canned response would
/// be consumed — Beta surviving proves the second chunk ran separately.
#[tokio::test]
async fn prechunked_extracts_each_given_chunk_without_rechunking() {
    use crate::chunking::Segment;
    let r0 = "(entity<|>Alpha<|>organization<|>First chunk entity.<|>)##";
    let r1 = "(entity<|>Beta<|>organization<|>Second chunk entity.<|>)##";
    let backend = Arc::new(MockBackend::new(vec![r0.into(), r1.into()]));
    let mut ex = SimpleExtractor::new(backend);
    ex.max_gleanings = 0;

    let chunks: Vec<Segment> = [(0usize, "Alpha exists."), (1, "Beta exists.")]
        .into_iter()
        .map(|(i, t)| Segment {
            content: t.to_string(),
            index: i,
            start: i * 13,
            end: (i + 1) * 13,
            lines: None,
        })
        .collect();
    let out = ex.extract_prechunked(&chunks).await.unwrap();

    let labels: Vec<&str> = out
        .knowledge_graph
        .entities
        .values()
        .map(|e| e.label.as_str())
        .collect();
    assert!(labels.contains(&"Alpha"), "chunk 0 ran: {labels:?}");
    assert!(
        labels.contains(&"Beta"),
        "chunk 1 must run as its own chunk, not be re-chunked away: {labels:?}"
    );
}

#[tokio::test]
async fn prechunked_empty_input_errors() {
    let backend = Arc::new(MockBackend::new(vec![]));
    let ex = SimpleExtractor::new(backend);
    assert!(ex.extract_prechunked(&[]).await.is_err());
}

/// Pre-chunked provenance: line ranges come from the chunks' own metadata
/// (chonkie's `start_line`/`end_line`), the doc from `source_doc`; a chunk
/// without line info contributes records unstamped.
#[tokio::test]
async fn prechunked_citations_use_chunk_metadata_lines() {
    use crate::chunking::Segment;
    use crate::citation::CITATIONS_KEY;

    let r0 = "(entity<|>Alpha<|>organization<|>First chunk entity.<|>)##";
    let r1 = "(entity<|>Beta<|>organization<|>Second chunk entity.<|>)##";
    let backend = Arc::new(MockBackend::new(vec![r0.into(), r1.into()]));
    let mut cfg = SimpleExtractor::default_config();
    cfg.max_concurrency = 1; // sequential → deterministic response order
    cfg.source_doc = Some("doc.md".into());
    let mut ex = SimpleExtractor::with_config(backend, cfg);
    ex.max_gleanings = 0;

    let chunks = vec![
        Segment {
            content: "Alpha exists.".into(),
            index: 0,
            start: 0,
            end: 13,
            lines: Some((5, 9)),
        },
        Segment {
            content: "Beta exists.".into(),
            index: 1,
            start: 13,
            end: 25,
            lines: None, // no metadata → no stamp
        },
    ];
    let out = ex.extract_prechunked(&chunks).await.unwrap();

    let by_label = |label: &str| {
        out.knowledge_graph
            .entities
            .values()
            .find(|e| e.label == label)
            .unwrap_or_else(|| panic!("{label} missing"))
            .metadata
            .get(CITATIONS_KEY)
            .cloned()
    };
    let alpha = by_label("Alpha").expect("Alpha must be stamped");
    assert_eq!(
        alpha,
        serde_json::json!([{"doc": "doc.md", "lines": [5, 9]}])
    );
    assert!(
        by_label("Beta").is_none(),
        "a chunk without line metadata must not be stamped"
    );
}

/// End-to-end provenance: chunked extraction stamps every record with the
/// chunk's doc + line range, and merging a duplicate entity unions the
/// citations from both chunks.
#[tokio::test]
async fn citations_stamp_chunk_line_ranges_and_union_on_merge() {
    use crate::citation::CITATIONS_KEY;
    use crate::types::ChunkStrategy;

    // Both chunks mention Alpha → after dedup its citations must cover both
    // chunk ranges; Beta appears only in chunk 1.
    let r0 = "(entity<|>Alpha<|>organization<|>First chunk entity.<|>)##";
    let r1 = "(entity<|>Alpha<|>organization<|>Also here.<|>)##\
        (entity<|>Beta<|>organization<|>Second chunk entity.<|>)##";
    let backend = Arc::new(MockBackend::new(vec![r0.into(), r1.into()]));

    let mut cfg = SimpleExtractor::default_config();
    cfg.segment_size = 20;
    cfg.overlap = 0;
    cfg.min_segment_size = 1;
    cfg.chunker = ChunkStrategy::Char;
    cfg.max_concurrency = 1; // sequential → deterministic response order
    cfg.source_doc = Some("doc.md".into());
    let mut ex = SimpleExtractor::with_config(backend, cfg);
    ex.max_gleanings = 0;

    // 4 lines of 10 chars each ("aaaaaaaaa\n" ×2 + "bbbbbbbbb\n" ×2) → two
    // 20-char Char chunks: chunk 0 = lines 1-2, chunk 1 = lines 3-4.
    let text = format!(
        "{}\n{}\n{}\n{}\n",
        "a".repeat(9),
        "a".repeat(9),
        "b".repeat(9),
        "b".repeat(9)
    );
    let out = ex.extract(&text).await.unwrap();

    let by_label = |label: &str| {
        out.knowledge_graph
            .entities
            .values()
            .find(|e| e.label == label)
            .unwrap_or_else(|| panic!("{label} missing"))
            .metadata
            .get(CITATIONS_KEY)
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("{label} has no citations"))
            .clone()
    };

    let alpha = by_label("Alpha");
    assert_eq!(
        alpha.len(),
        2,
        "Alpha in both chunks must cite both: {alpha:?}"
    );
    assert_eq!(alpha[0]["doc"], "doc.md");
    assert_eq!(alpha[0]["lines"], serde_json::json!([1, 2]));
    assert_eq!(alpha[1]["lines"], serde_json::json!([3, 4]));

    let beta = by_label("Beta");
    assert_eq!(beta.len(), 1);
    assert_eq!(beta[0]["lines"], serde_json::json!([3, 4]));
}

#[test]
fn entity_type_tokens_recovers_raw_types() {
    // Recovers the literal type token (keyed by lowercased name), including a
    // custom one the EntityType enum would collapse to `Other`. Relationship
    // records are ignored.
    let output = "(entity<|>OpenAI<|>organization<|>An AI lab.<|>)##\
        (entity<|>Widget X<|>GADGET<|>A custom thing.<|>)##\
        (relationship<|>OpenAI<|>Widget X<|>builds<|>they build it<|>0.9)";
    let tokens = entity_type_tokens(output);
    assert_eq!(
        tokens.get("openai").map(String::as_str),
        Some("organization")
    );
    assert_eq!(tokens.get("widget x").map(String::as_str), Some("GADGET"));
    assert_eq!(tokens.len(), 2, "relationship record must not appear");
}
