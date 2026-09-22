use super::*;

#[test]
fn line_index_maps_multibyte_text_by_chars() {
    // 3 lines of Chinese; offsets are chars, so line 2 starts at char 6
    // (5 chars + '\n'), NOT at a byte boundary.
    let text = "第一行内容\n第二行内容\n第三行";
    let li = LineIndex::new(text);
    assert_eq!(li.total_lines(), 3);
    assert_eq!(li.line_of(0), 1);
    assert_eq!(li.line_of(5), 1); // the '\n' belongs to line 1
    assert_eq!(li.line_of(6), 2);
    assert_eq!(li.line_of(12), 3);
    assert_eq!(li.line_range(0, 11), (1, 2)); // chars 0..11 span lines 1-2
    assert_eq!(li.line_range(12, 15), (3, 3));
}

#[test]
fn line_range_of_empty_slice_cites_its_start_line() {
    let li = LineIndex::new("a\nb\nc");
    assert_eq!(li.line_range(2, 2), (2, 2));
}

#[test]
fn stamp_chunk_metadata_attaches_title_and_metadata_to_every_record() {
    use crate::chunking::Segment;
    use crate::types::{Entity, EntityType, Predicate, PredicateType, Triple};
    use std::collections::BTreeMap;

    let mut chunk_meta = BTreeMap::new();
    chunk_meta.insert("mm_kind".to_string(), json!("table"));
    let seg = Segment {
        content: "Revenue grew.".into(),
        index: 0,
        start: 0,
        end: 13,
        range: None,
        title: Some("q4_revenue".into()),
        metadata: chunk_meta,
    };

    let mut kg = KnowledgeGraph::new();
    kg.add_entity(Entity::new("e1", "Alpha", EntityType::Organization));
    kg.add_triple(Triple::new(
        Entity::new("e1", "Alpha", EntityType::Organization),
        Predicate::new(PredicateType::RelatedTo),
        Entity::new("e2", "Beta", EntityType::Technology),
    ));
    stamp_chunk_metadata(&mut kg, &seg);

    let stored = kg.get_entity("e1").unwrap();
    assert_eq!(stored.metadata[CHUNK_TITLE_KEY], json!("q4_revenue"));
    assert_eq!(
        stored.metadata[CHUNK_METADATA_KEY],
        json!({"mm_kind": "table"})
    );
    assert_eq!(kg.triples[0].metadata[CHUNK_TITLE_KEY], json!("q4_revenue"));
    // endpoint snapshots are stamped too (add_triple re-inserts them)
    assert_eq!(
        kg.triples[0].object.metadata[CHUNK_METADATA_KEY],
        json!({"mm_kind": "table"})
    );
}

#[test]
fn stamp_chunk_metadata_is_noop_without_payload() {
    use crate::chunking::Segment;
    use crate::types::{Entity, EntityType};
    use std::collections::BTreeMap;

    let seg = Segment {
        content: "plain".into(),
        index: 0,
        start: 0,
        end: 5,
        range: None,
        title: None,
        metadata: BTreeMap::new(),
    };
    let mut kg = KnowledgeGraph::new();
    kg.add_entity(Entity::new("e1", "Alpha", EntityType::Organization));
    stamp_chunk_metadata(&mut kg, &seg);
    assert!(!kg.get_entity("e1").unwrap().metadata.contains_key(CHUNK_TITLE_KEY));
    assert!(!kg
        .get_entity("e1")
        .unwrap()
        .metadata
        .contains_key(CHUNK_METADATA_KEY));
}

#[test]
fn attach_deduplicates_identical_citations() {
    let mut meta = HashMap::new();
    let c = Citation::new(Some("doc.md".into()), 3, 7);
    attach_citation(&mut meta, &c);
    attach_citation(&mut meta, &c);
    let list = meta.get(CITATIONS_KEY).and_then(|v| v.as_array()).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0], json!({"doc": "doc.md", "lines": [3, 7]}));
}

#[test]
fn union_merges_distinct_and_skips_duplicate_citations() {
    let mut a = HashMap::new();
    let mut b = HashMap::new();
    attach_citation(&mut a, &Citation::new(Some("x.md".into()), 1, 4));
    attach_citation(&mut b, &Citation::new(Some("x.md".into()), 1, 4)); // dup
    attach_citation(&mut b, &Citation::new(Some("y.md".into()), 9, 12)); // new
    union_citations(&mut a, &b);
    let list = a.get(CITATIONS_KEY).and_then(|v| v.as_array()).unwrap();
    assert_eq!(list.len(), 2);
}

#[test]
fn add_triple_endpoint_overwrite_keeps_entity_citations() {
    use crate::types::{Entity, EntityType, Predicate, PredicateType, Triple};

    // Regression: add_triple re-inserts endpoint snapshots into the entity
    // table; a snapshot without citations must not erase the provenance
    // already recorded on the stored entity.
    let mut kg = KnowledgeGraph::new();
    let mut a = Entity::new("e1", "Alpha", EntityType::Organization);
    attach_citation(&mut a.metadata, &Citation::new(Some("doc.md".into()), 1, 4));
    kg.add_entity(a.clone());

    let bare_a = Entity::new("e1", "Alpha", EntityType::Organization); // no citations
    let b = Entity::new("e2", "Beta", EntityType::Organization);
    let t = Triple::new(bare_a, Predicate::new(PredicateType::RelatedTo), b);
    kg.add_triple(t);

    let stored = kg.get_entity("e1").unwrap();
    let cites = stored
        .metadata
        .get(CITATIONS_KEY)
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(
        cites.len(),
        1,
        "endpoint overwrite must keep prior citations"
    );
}

#[test]
fn union_into_empty_dst_copies_all() {
    let mut a = HashMap::new();
    let mut b = HashMap::new();
    attach_citation(&mut b, &Citation::new(None, 2, 2));
    union_citations(&mut a, &b);
    assert!(a.contains_key(CITATIONS_KEY));
}

#[test]
fn stamp_whole_document_covers_full_line_range_on_every_record() {
    use crate::types::{Entity, EntityType, Predicate, PredicateType, Triple};

    // 3 lines -> the whole-document citation must span [1, 3].
    let text = "alpha line\nbeta line\ngamma line";
    assert_eq!(LineIndex::new(text).total_lines(), 3);

    let mut kg = KnowledgeGraph::new();
    kg.add_entity(Entity::new("e1", "Alpha", EntityType::Organization));
    kg.add_entity(Entity::new("e2", "Beta", EntityType::Technology));
    kg.add_triple(Triple::new(
        Entity::new("e1", "Alpha", EntityType::Organization),
        Predicate::new(PredicateType::RelatedTo),
        Entity::new("e2", "Beta", EntityType::Technology),
    ));

    stamp_whole_document(&mut kg, &Some("doc.md".into()), text);

    let expected = json!({"doc": "doc.md", "lines": [1, 3]});
    for e in kg.entities.values() {
        assert_eq!(
            e.metadata
                .get(CITATIONS_KEY)
                .and_then(|v| v.as_array())
                .and_then(|a| a.first()),
            Some(&expected),
            "entity {} must carry the whole-doc citation",
            e.label
        );
    }
    for t in &kg.triples {
        assert_eq!(
            t.metadata
                .get(CITATIONS_KEY)
                .and_then(|v| v.as_array())
                .and_then(|a| a.first()),
            Some(&expected),
            "triple must carry the whole-doc citation"
        );
        for endpoint in [&t.subject, &t.object] {
            assert_eq!(
                endpoint
                    .metadata
                    .get(CITATIONS_KEY)
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first()),
                Some(&expected),
                "triple endpoint {} must carry the whole-doc citation",
                endpoint.label
            );
        }
    }
}
