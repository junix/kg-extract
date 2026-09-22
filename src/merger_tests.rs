use super::*;
use crate::types::{EntityType, Predicate, PredicateType};

#[test]
fn dedup_collision_rewrites_entity_id_to_new_key() {
    // g1 has "e1"=Alice; g2 reuses "e1" for a different entity (Bob) plus a
    // Bob->Paris relation. Bob must land under a fresh key whose value's `.id`
    // equals that key, and the remapped triple must carry the fresh id — not
    // the colliding "e1" that belongs to Alice.
    let mut g1 = KnowledgeGraph::new();
    g1.add_entity(Entity::new("e1", "Alice", EntityType::Person));

    let bob = Entity::new("e1", "Bob", EntityType::Person);
    let paris = Entity::new("e3", "Paris", EntityType::City);
    let mut g2 = KnowledgeGraph::new();
    g2.add_entity(bob.clone());
    g2.add_entity(paris.clone());
    g2.add_triple(Triple::new(
        bob.clone(),
        Predicate::new(PredicateType::LocatedIn),
        paris,
    ));

    let merged = merge_with_deduplication(g1, g2);

    // Table key and entity.id must agree for every entity.
    for (key, e) in merged.entities.iter() {
        assert_eq!(
            &e.id, key,
            "entity '{}' stored under key '{}' has stale id",
            e.label, key
        );
    }
    let bob_key = merged
        .entities
        .iter()
        .find(|(_, e)| e.label == "Bob")
        .map(|(k, _)| k.clone())
        .expect("Bob present");
    assert_ne!(bob_key, "e1", "Bob must get a fresh id, not Alice's e1");
    // The Bob->Paris triple endpoint must carry Bob's fresh id.
    let t = merged
        .triples
        .iter()
        .find(|t| t.subject.label == "Bob")
        .expect("Bob relation kept");
    assert_eq!(
        t.subject.id, bob_key,
        "triple subject id must match Bob's table key"
    );
}

#[test]
fn field_union_combines_description_confidence_metadata_and_type() {
    let mut a = Entity::new("e1", "Acme", EntityType::Organization);
    a.description = Some("short".into());
    a.confidence = Some(0.5);
    a.metadata.insert("x".into(), serde_json::json!(1));
    let mut b = Entity::new("e2", "acme", EntityType::Other);
    b.description = Some("a much longer, richer description".into());
    b.confidence = Some(0.9);
    b.metadata.insert("y".into(), serde_json::json!(2));

    let merged = combine_entities(MergeStrategy::FieldUnion, &a, &b, None);
    assert_eq!(merged.id, "e1", "must keep the canonical (existing) id");
    assert_eq!(
        merged.description.as_deref(),
        Some("a much longer, richer description")
    );
    assert_eq!(
        merged.confidence,
        Some(0.9),
        "confidence is the max of both"
    );
    assert!(merged.metadata.contains_key("x") && merged.metadata.contains_key("y"));
    assert_eq!(
        merged.entity_type,
        EntityType::Organization,
        "specific type beats Other"
    );
}

#[test]
fn keep_incoming_replaces_but_preserves_canonical_id() {
    let a = Entity::new("e1", "Acme", EntityType::Organization);
    let mut b = Entity::new("e2", "Acme", EntityType::City);
    b.description = Some("new".into());
    let merged = combine_entities(MergeStrategy::KeepIncoming, &a, &b, None);
    assert_eq!(merged.id, "e1");
    assert_eq!(merged.entity_type, EntityType::City);
    assert_eq!(merged.description.as_deref(), Some("new"));
}

#[test]
fn field_union_dedup_keeps_the_richer_description() {
    let mut g1 = KnowledgeGraph::new();
    let mut a = Entity::new("e1", "Acme", EntityType::Organization);
    a.description = Some("HQ in NY".into());
    g1.add_entity(a);
    let mut g2 = KnowledgeGraph::new();
    let mut b = Entity::new("e9", "acme", EntityType::Organization);
    b.description = Some("a global manufacturing company".into());
    g2.add_entity(b);

    let merged = merge_with_deduplication_strategy(g1, g2, MergeStrategy::FieldUnion);
    assert_eq!(merged.entities.len(), 1, "same-label entities collapse");
    let e = merged.entities.values().next().unwrap();
    assert_eq!(
        e.description.as_deref(),
        Some("a global manufacturing company")
    );
}

#[test]
fn normalize_label_strips_punctuation_articles_and_suffixes() {
    assert_eq!(normalize_label("OpenAI"), "openai");
    assert_eq!(normalize_label("Open AI"), "open ai");
    assert_eq!(normalize_label("Anthropic, PBC"), "anthropic");
    assert_eq!(normalize_label("Google Inc."), "google");
    assert_eq!(normalize_label("The New York Times"), "new york times");
    // A bare suffix token is never stripped to empty.
    assert_eq!(normalize_label("Inc"), "inc");
}

#[test]
fn coref_off_keeps_surface_variants_separate() {
    let mut g1 = KnowledgeGraph::new();
    g1.add_entity(Entity::new("e1", "Anthropic", EntityType::Company));
    let mut g2 = KnowledgeGraph::new();
    g2.add_entity(Entity::new("e2", "Anthropic, PBC", EntityType::Company));

    let merged =
        merge_with_deduplication_strategy_coref(g1, g2, MergeStrategy::FieldUnion, CorefMode::Off);
    assert_eq!(merged.entities.len(), 2, "Off mode must not fuse variants");
}

#[test]
fn coref_fuzzy_merges_normalized_variant_and_remaps_triples() {
    // g1: "Anthropic" founded by a person. g2 calls it "Anthropic, PBC" and
    // adds another relation. Fuzzy coref must collapse the two company nodes
    // and rebind every triple endpoint onto the surviving id.
    let anthropic = Entity::new("e1", "Anthropic", EntityType::Company);
    let dario = Entity::new("e2", "Dario", EntityType::Person);
    let mut g1 = KnowledgeGraph::new();
    g1.add_entity(anthropic.clone());
    g1.add_entity(dario.clone());
    g1.add_triple(Triple::new(
        anthropic,
        Predicate::new(PredicateType::FoundedBy),
        dario,
    ));

    let anthropic2 = Entity::new("e9", "Anthropic, PBC", EntityType::Company);
    let claude = Entity::new("e8", "Claude", EntityType::Technology);
    let mut g2 = KnowledgeGraph::new();
    g2.add_entity(anthropic2.clone());
    g2.add_entity(claude.clone());
    g2.add_triple(Triple::new(
        anthropic2,
        Predicate::new(PredicateType::DevelopedBy),
        claude,
    ));

    let merged = merge_with_deduplication_strategy_coref(
        g1,
        g2,
        MergeStrategy::FieldUnion,
        CorefMode::Fuzzy,
    );
    // Anthropic(+PBC) collapse → 3 entities: Anthropic, Dario, Claude.
    assert_eq!(
        merged.entities.len(),
        3,
        "the two Anthropic nodes must fuse"
    );
    assert!(merged.entities.contains_key("e1"), "canonical id survives");
    // Both relations now reference the canonical Anthropic id e1. (DevelopedBy
    // swaps actor→artifact, so Anthropic is the object there.)
    assert_eq!(merged.triples.len(), 2);
    for t in &merged.triples {
        assert!(
            t.subject.id == "e1" || t.object.id == "e1",
            "every triple must touch the canonical Anthropic id"
        );
    }
}

#[test]
fn coref_fuzzy_merges_near_typo_but_respects_type_and_length() {
    // Long near-identical, type-compatible labels fuse...
    let mut g1 = KnowledgeGraph::new();
    g1.add_entity(Entity::new("e1", "Anthropic", EntityType::Company));
    let mut g2 = KnowledgeGraph::new();
    g2.add_entity(Entity::new("e2", "Antropic", EntityType::Company));
    let merged = merge_with_deduplication_strategy_coref(
        g1,
        g2,
        MergeStrategy::FieldUnion,
        CorefMode::Fuzzy,
    );
    assert_eq!(merged.entities.len(), 1, "near-typo of a long name fuses");

    // ...but short labels never fuzzy-fuse (too collision-prone).
    let mut g3 = KnowledgeGraph::new();
    g3.add_entity(Entity::new("e1", "Go", EntityType::Technology));
    let mut g4 = KnowledgeGraph::new();
    g4.add_entity(Entity::new("e2", "Rust", EntityType::Technology));
    let merged = merge_with_deduplication_strategy_coref(
        g3,
        g4,
        MergeStrategy::FieldUnion,
        CorefMode::Fuzzy,
    );
    assert_eq!(merged.entities.len(), 2, "short labels stay distinct");
}

#[test]
fn token_set_similarity_subset_and_jaccard() {
    // Multi-token containment fuses.
    assert_eq!(
        token_set_similarity("new york", "new york times"),
        Some(1.0)
    );
    // Non-subset token overlap reaches the Jaccard threshold (3 of 5).
    let s = token_set_similarity("alpha beta gamma delta", "alpha beta gamma epsilon").unwrap();
    assert!((s - 0.6).abs() < 1e-9);
    // Single-token containment is rejected (too generic).
    assert_eq!(token_set_similarity("apple", "apple store"), None);
    // Disjoint token sets never match.
    assert_eq!(token_set_similarity("acme", "globex"), None);
}

#[test]
fn coref_fuzzy_token_set_merges_subset_and_jaccard_surfaces() {
    // Subset relation: "New York" ⊂ "New York Times" (the edit-distance
    // channel cannot fuse these: similarity is only 0.62).
    let mut g1 = KnowledgeGraph::new();
    g1.add_entity(Entity::new(
        "e1",
        "New York Times",
        EntityType::Organization,
    ));
    let mut g2 = KnowledgeGraph::new();
    g2.add_entity(Entity::new("e2", "New York", EntityType::Organization));
    let merged = merge_with_deduplication_strategy_coref(
        g1,
        g2,
        MergeStrategy::FieldUnion,
        CorefMode::Fuzzy,
    );
    assert_eq!(merged.entities.len(), 1, "multi-token subset fuses");
    assert!(merged.entities.contains_key("e1"));

    // Token-overlap path: "CSAIL AI Lab" vs "CSAIL Lab" (subset: "csail lab"
    // ⊂ "csail ai lab") — edit distance alone (0.75) cannot fuse these.
    let mut g1 = KnowledgeGraph::new();
    g1.add_entity(Entity::new("e1", "CSAIL AI Lab", EntityType::Organization));
    let mut g2 = KnowledgeGraph::new();
    g2.add_entity(Entity::new("e2", "CSAIL Lab", EntityType::Organization));
    let merged = merge_with_deduplication_strategy_coref(
        g1,
        g2,
        MergeStrategy::FieldUnion,
        CorefMode::Fuzzy,
    );
    assert_eq!(merged.entities.len(), 1, "token-overlapping surfaces fuse");
}

#[test]
fn coref_fuzzy_token_set_respects_type_gate_and_short_names() {
    // Same subset relation, but incompatible types (City vs Organization)
    // must not fuse.
    let mut g1 = KnowledgeGraph::new();
    g1.add_entity(Entity::new(
        "e1",
        "New York Times",
        EntityType::Organization,
    ));
    let mut g2 = KnowledgeGraph::new();
    g2.add_entity(Entity::new("e2", "New York", EntityType::City));
    let merged = merge_with_deduplication_strategy_coref(
        g1,
        g2,
        MergeStrategy::FieldUnion,
        CorefMode::Fuzzy,
    );
    assert_eq!(merged.entities.len(), 2, "incompatible types stay apart");

    // Single-token containment ("Apple" ⊂ "Apple Store") is too generic to
    // fuse — and too dissimilar for edit distance.
    let mut g3 = KnowledgeGraph::new();
    g3.add_entity(Entity::new("e1", "Apple Store", EntityType::Organization));
    let mut g4 = KnowledgeGraph::new();
    g4.add_entity(Entity::new("e2", "Apple", EntityType::Organization));
    let merged = merge_with_deduplication_strategy_coref(
        g3,
        g4,
        MergeStrategy::FieldUnion,
        CorefMode::Fuzzy,
    );
    assert_eq!(merged.entities.len(), 2, "one-token subset stays apart");
}

#[test]
fn coref_fuzzy_token_set_is_deterministic_earliest_wins() {
    // "New York" is a subset of BOTH seeded labels; the tie must resolve
    // deterministically to the earliest-inserted entity, on every run.
    let build = || {
        let mut g1 = KnowledgeGraph::new();
        g1.add_entity(Entity::new(
            "e1",
            "New York Times",
            EntityType::Organization,
        ));
        g1.add_entity(Entity::new("e9", "New York Post", EntityType::Organization));
        let mut g2 = KnowledgeGraph::new();
        g2.add_entity(Entity::new("e2", "New York", EntityType::Organization));
        merge_with_deduplication_strategy_coref(g1, g2, MergeStrategy::FieldUnion, CorefMode::Fuzzy)
    };
    let first = build();
    assert_eq!(first.entities.len(), 2);
    assert!(
        first.entities.contains_key("e1") && first.entities.contains_key("e9"),
        "incoming must fuse into the earliest-inserted candidate"
    );
    for _ in 0..8 {
        let again = build();
        assert_eq!(
            again.entities.keys().collect::<Vec<_>>(),
            first.entities.keys().collect::<Vec<_>>(),
            "repeated merges must pick the same canonical entity"
        );
    }
}

#[test]
fn coref_fuzzy_merges_abbreviation_with_suffixed_form() {
    // The motivating alias case: "ACME" and "Acme Corporation" normalize
    // to the same token string and must collapse under Fuzzy.
    let mut g1 = KnowledgeGraph::new();
    g1.add_entity(Entity::new(
        "e1",
        "Acme Corporation",
        EntityType::Organization,
    ));
    let mut g2 = KnowledgeGraph::new();
    g2.add_entity(Entity::new("e2", "ACME", EntityType::Organization));
    let merged = merge_with_deduplication_strategy_coref(
        g1,
        g2,
        MergeStrategy::FieldUnion,
        CorefMode::Fuzzy,
    );
    assert_eq!(merged.entities.len(), 1);
    assert!(merged.entities.contains_key("e1"));
}

#[test]
fn canonical_predicate_picks_first_declared_member_of_pair() {
    // Declaration-order rule: the variant declared first in vocab.json is
    // canonical. IS_USED_BY precedes USES, so that pair canonicalises to
    // IS_USED_BY; PART_OF precedes COMPOSED_OF. Unpaired predicates map to
    // themselves.
    assert_eq!(
        canonical_predicate(PredicateType::Uses),
        PredicateType::IsUsedBy
    );
    assert_eq!(
        canonical_predicate(PredicateType::IsUsedBy),
        PredicateType::IsUsedBy
    );
    assert_eq!(
        canonical_predicate(PredicateType::ComposedOf),
        PredicateType::PartOf
    );
    assert_eq!(
        canonical_predicate(PredicateType::PartOf),
        PredicateType::PartOf
    );
    assert_eq!(
        canonical_predicate(PredicateType::Precedes),
        PredicateType::Succeeds
    );
    assert_eq!(
        canonical_predicate(PredicateType::Measures),
        PredicateType::MeasuredBy
    );
    assert_eq!(
        canonical_predicate(PredicateType::RelatedTo),
        PredicateType::RelatedTo
    );
}

#[test]
fn normalize_direction_flips_noncanonical_member_only() {
    let a = Entity::new("e1", "Alpha", EntityType::Organization);
    let b = Entity::new("e2", "Beta", EntityType::Technology);
    let mut kg = KnowledgeGraph::new();
    kg.add_triple(Triple::new(
        a.clone(),
        Predicate::with_label(PredicateType::Uses, "uses"),
        b.clone(),
    ));
    kg.add_triple(Triple::new(
        b.clone(),
        Predicate::new(PredicateType::IsUsedBy),
        a.clone(),
    ));
    kg.add_triple(Triple::new(a, Predicate::new(PredicateType::RelatedTo), b));

    normalize_direction(&mut kg);

    let keys: Vec<_> = kg.triples.iter().map(|t| t.to_tuple()).collect();
    assert_eq!(
        keys,
        vec![
            ("e2".to_string(), "IS_USED_BY".to_string(), "e1".to_string()),
            ("e2".to_string(), "IS_USED_BY".to_string(), "e1".to_string()),
            ("e1".to_string(), "RELATED_TO".to_string(), "e2".to_string()),
        ],
        "USES flips to canonical IS_USED_BY with swapped endpoints; \
             the unpaired RELATED_TO is untouched"
    );
    let flipped = &kg.triples[0];
    assert_eq!(flipped.subject.label, "Beta");
    assert_eq!(flipped.object.label, "Alpha");
    // The stale surface token is cleared (display falls back to the
    // canonical enum value); the original token survives in metadata.
    assert_eq!(flipped.predicate.raw_type, None);
    assert_eq!(flipped.predicate.label, None);
    assert_eq!(flipped.predicate.output_type(), "IS_USED_BY");
    assert_eq!(
        flipped.predicate.metadata["direction_normalized_from"],
        serde_json::json!("uses")
    );
    // An untouched triple keeps its fields.
    assert_eq!(
        kg.triples[2].predicate.predicate_type,
        PredicateType::RelatedTo
    );
}

#[test]
fn direction_variants_dedup_to_one_edge_after_normalization() {
    // g1: (Alpha, USES, Beta); g2: (Beta, IS_USED_BY, Alpha) — the same
    // semantic edge in two directions. With canonical direction applied
    // before the merge, they collapse to ONE triple and union citations.
    let alpha = Entity::new("e1", "Alpha", EntityType::Organization);
    let beta = Entity::new("e2", "Beta", EntityType::Technology);
    let mut g1 = KnowledgeGraph::new();
    g1.add_entity(alpha.clone());
    g1.add_entity(beta.clone());
    let mut t1 = Triple::new(
        alpha.clone(),
        Predicate::new(PredicateType::Uses),
        beta.clone(),
    );
    crate::citation::attach_citation(
        &mut t1.metadata,
        &crate::citation::Citation::new(None, 1, 2),
    );
    g1.add_triple(t1);

    let mut g2 = KnowledgeGraph::new();
    g2.add_entity(alpha.clone());
    g2.add_entity(beta.clone());
    let mut t2 = Triple::new(beta, Predicate::new(PredicateType::IsUsedBy), alpha);
    crate::citation::attach_citation(
        &mut t2.metadata,
        &crate::citation::Citation::new(None, 5, 6),
    );
    g2.add_triple(t2);

    normalize_direction(&mut g1);
    normalize_direction(&mut g2);
    let merged = merge_with_deduplication(g1, g2);

    assert_eq!(
        merged.triples.len(),
        1,
        "direction variants share one dedup key after normalisation"
    );
    let t = &merged.triples[0];
    assert_eq!(t.predicate.predicate_type, PredicateType::IsUsedBy);
    // Provenance from BOTH direction variants survives on the kept edge.
    let citations = t.metadata["citations"].as_array().unwrap();
    assert_eq!(citations.len(), 2, "citations union across the variants");
}

#[test]
fn direction_variants_survive_dedup_when_normalization_off() {
    // Default behaviour (normalisation off): the two direction variants
    // remain distinct edges — pinned so the opt-in cannot silently become
    // the default.
    let alpha = Entity::new("e1", "Alpha", EntityType::Organization);
    let beta = Entity::new("e2", "Beta", EntityType::Technology);
    let mut g1 = KnowledgeGraph::new();
    g1.add_entity(alpha.clone());
    g1.add_entity(beta.clone());
    g1.add_triple(Triple::new(
        alpha.clone(),
        Predicate::new(PredicateType::Uses),
        beta.clone(),
    ));
    let mut g2 = KnowledgeGraph::new();
    g2.add_entity(alpha.clone());
    g2.add_entity(beta.clone());
    g2.add_triple(Triple::new(
        beta,
        Predicate::new(PredicateType::IsUsedBy),
        alpha,
    ));

    let merged = merge_with_deduplication(g1, g2);
    assert_eq!(
        merged.triples.len(),
        2,
        "without normalisation, USES and IS_USED_BY stay separate edges"
    );
}

#[test]
fn normalize_direction_is_deterministic() {
    let build = || {
        let a = Entity::new("e1", "Alpha", EntityType::Organization);
        let b = Entity::new("e2", "Beta", EntityType::Technology);
        let c = Entity::new("e3", "Gamma", EntityType::Technology);
        let mut kg = KnowledgeGraph::new();
        kg.add_triple(Triple::new(
            a.clone(),
            Predicate::new(PredicateType::Uses),
            b.clone(),
        ));
        kg.add_triple(Triple::new(
            b.clone(),
            Predicate::new(PredicateType::IsUsedBy),
            a.clone(),
        ));
        kg.add_triple(Triple::new(
            b,
            Predicate::new(PredicateType::ComposedOf),
            c.clone(),
        ));
        kg.add_triple(Triple::new(
            c,
            Predicate::new(PredicateType::DerivesFrom),
            a,
        ));
        normalize_direction(&mut kg);
        kg
    };
    let first = build();
    for _ in 0..8 {
        let again = build();
        assert_eq!(
            again
                .triples
                .iter()
                .map(|t| t.to_tuple())
                .collect::<Vec<_>>(),
            first
                .triples
                .iter()
                .map(|t| t.to_tuple())
                .collect::<Vec<_>>(),
            "normalisation is a pure per-triple transform — same input, same output"
        );
    }
}

#[tokio::test]
async fn llm_strategy_synthesizes_merged_description() {
    use crate::backend::MockBackend;
    let synthesized = "Acme: a global manufacturer headquartered in NY.";
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::single(synthesized));
    let opts = CompletionOptions::default();

    let mut g1 = KnowledgeGraph::new();
    let mut a = Entity::new("e1", "Acme", EntityType::Organization);
    a.description = Some("HQ in NY".into());
    g1.add_entity(a);
    let mut g2 = KnowledgeGraph::new();
    let mut b = Entity::new("e2", "Acme", EntityType::Organization);
    b.description = Some("global manufacturer".into());
    g2.add_entity(b);

    let merged = merge_knowledge_graphs_llm(g1, g2, &backend, &opts).await;
    assert_eq!(merged.entities.len(), 1);
    let e = merged.entities.values().next().unwrap();
    assert_eq!(e.description.as_deref(), Some(synthesized));
}
