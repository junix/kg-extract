use super::*;
use crate::types::EntityType;

fn ent(id: &str, label: &str, conf: Option<f64>) -> Entity {
    let mut e = Entity::new(id, label, EntityType::Other);
    e.confidence = conf;
    e
}
fn tri(s: &Entity, p: PredicateType, o: &Entity) -> Triple {
    Triple::new(s.clone(), Predicate::new(p), o.clone())
}

#[test]
fn type_normalization_report_none_when_all_exact() {
    let mut a = Entity::new("e1", "Ada", EntityType::Person);
    a.raw_type = Some("PERSON".into());
    let mut b = Entity::new("e2", "Rust", EntityType::Technology);
    b.raw_type = Some("TECHNOLOGY".into());
    let mut g = KnowledgeGraph::new();
    g.add_entity(a.clone());
    g.add_entity(b.clone());
    g.add_triple(Triple::new(
        a,
        Predicate::with_label(PredicateType::Uses, "USES"),
        b,
    ));
    assert!(g.type_normalization_report().is_none());
}

#[test]
fn type_normalization_report_records_aliased_and_fallback() {
    // An aliased entity token (LLM → TECHNOLOGY) and a genuinely unknown one.
    let mut model = Entity::new("e1", "Claude", EntityType::Technology);
    model.raw_type = Some("LLM".into());
    let mut blob = Entity::new("e2", "Mystery", EntityType::Other);
    blob.raw_type = Some("WIDGET".into());
    let mut g = KnowledgeGraph::new();
    g.add_entity(model.clone());
    g.add_entity(blob.clone());
    // A relation token that only fuzzy-matches (aliased) and one that is lost.
    g.add_triple(Triple::new(
        model.clone(),
        Predicate::with_label(PredicateType::DevelopedBy, "is developed by"),
        blob.clone(),
    ));
    g.add_triple(Triple::new(
        blob,
        Predicate::with_label(PredicateType::RelatedTo, "frobnicates"),
        model,
    ));

    let report = g.type_normalization_report().expect("report present");
    assert_eq!(report["entities"]["aliased"]["LLM"], "TECHNOLOGY");
    assert_eq!(
        report["entities"]["fallback"],
        serde_json::json!(["WIDGET"])
    );
    assert_eq!(
        report["relations"]["aliased"]["is developed by"],
        "DEVELOPED_BY"
    );
    assert_eq!(
        report["relations"]["fallback"],
        serde_json::json!(["frobnicates"])
    );
    assert_eq!(report["counts"]["entities_aliased"], 1);
    assert_eq!(report["counts"]["entities_fallback"], 1);
    assert_eq!(report["counts"]["relations_aliased"], 1);
    assert_eq!(report["counts"]["relations_fallback"], 1);
}

#[test]
fn to_node_link_uses_source_target_referencing_node_ids() {
    let a = ent("e1", "A", Some(0.8));
    let b = ent("e2", "B", None);
    let mut g = KnowledgeGraph::new();
    g.add_triple(tri(&a, PredicateType::Uses, &b));

    let v = g.to_node_link();
    assert_eq!(v["directed"], serde_json::json!(true));

    let nodes = v["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes.len(), 2);
    // Nodes carry their id (the source/target join key).
    assert_eq!(nodes[0]["id"], "e1");
    assert_eq!(nodes[0]["label"], "A");

    let links = v["links"].as_array().expect("links array");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["source"], "e1");
    assert_eq!(links[0]["target"], "e2");
    assert_eq!(links[0]["type"], PredicateType::Uses.value());
    // No RDF-style keys leak into the node-link shape.
    assert!(links[0].get("subject").is_none());
    assert!(links[0].get("object").is_none());
}

#[test]
fn merge_dedups_identical_triples_within_other() {
    let a = ent("e1", "A", None);
    let b = ent("e2", "B", None);
    let mut other = KnowledgeGraph::new();
    other.add_entity(a.clone());
    other.add_entity(b.clone());
    // Two identical triples in `other` (e.g. an LLM segment that repeated a
    // relation). The dedup set was seeded once and never updated, so both
    // used to survive.
    other.triples.push(tri(&a, PredicateType::Uses, &b));
    other.triples.push(tri(&a, PredicateType::Uses, &b));

    let mut g = KnowledgeGraph::new();
    g.merge(other);
    assert_eq!(
        g.triples.len(),
        1,
        "identical triples within `other` must dedup"
    );
}

#[test]
fn to_tuple_uses_normalized_predicate_type_not_raw_surface_form() {
    // Same canonical predicate, different model-emitted surface tokens —
    // the dedup key must collapse them.
    let a = ent("e1", "A", None);
    let b = ent("e2", "B", None);
    let lower = Triple::new(
        a.clone(),
        Predicate::with_label(PredicateType::Uses, "uses"),
        b.clone(),
    );
    let upper = Triple::new(a, Predicate::with_label(PredicateType::Uses, "USES"), b);
    assert_eq!(lower.to_tuple(), upper.to_tuple());
    assert_eq!(lower.to_tuple().1, "USES");
}

#[test]
fn merge_dedups_surface_variant_predicates_across_chunks() {
    // Regression: chunk 1 emitted "uses", chunk 2 emitted "USES". The dedup
    // key used the raw surface token, so both edges survived the merge.
    let a = ent("e1", "A", None);
    let b = ent("e2", "B", None);
    let mut g = KnowledgeGraph::new();
    g.add_triple(Triple::new(
        a.clone(),
        Predicate::with_label(PredicateType::Uses, "uses"),
        b.clone(),
    ));

    let mut other = KnowledgeGraph::new();
    other.add_triple(Triple::new(
        a,
        Predicate::with_label(PredicateType::Uses, "USES"),
        b,
    ));

    g.merge(other);
    assert_eq!(
        g.triples.len(),
        1,
        "triples differing only in raw predicate surface form must dedup"
    );
}

#[test]
fn merge_keeps_distinct_normalized_predicates() {
    // Guard the other direction: same endpoints but different canonical
    // predicates must NOT dedup.
    let a = ent("e1", "A", None);
    let b = ent("e2", "B", None);
    let mut g = KnowledgeGraph::new();
    g.add_triple(Triple::new(
        a.clone(),
        Predicate::with_label(PredicateType::Uses, "uses"),
        b.clone(),
    ));
    g.add_triple(Triple::new(
        a,
        Predicate::with_label(PredicateType::DevelopedBy, "developed by"),
        b,
    ));
    assert_eq!(g.triples.len(), 2);
}

#[test]
fn merge_does_not_clobber_higher_confidence_entity_via_triple_endpoint() {
    // `self` has a rich, high-confidence X; `other` has a poor, low-confidence
    // X embedded as a triple endpoint. The confidence-based entity merge must
    // not be silently undone when the triple is added.
    let mut x_rich = ent("x", "X", Some(0.9));
    x_rich.description = Some("rich".into());
    let mut g = KnowledgeGraph::new();
    g.add_entity(x_rich);

    let x_poor = ent("x", "X", Some(0.1));
    let y = ent("y", "Y", None);
    let mut other = KnowledgeGraph::new();
    other.add_entity(x_poor.clone());
    other.add_entity(y.clone());
    other.add_triple(tri(&x_poor, PredicateType::Uses, &y));

    g.merge(other);
    let x = g.get_entity("x").expect("x present");
    assert_eq!(
        x.confidence,
        Some(0.9),
        "higher-confidence entity must survive"
    );
    assert_eq!(
        x.description.as_deref(),
        Some("rich"),
        "rich entity must not be clobbered by a stale triple endpoint"
    );
    // The merged triple's endpoint must also reflect the canonical entity,
    // not the poor snapshot that came embedded in `other`'s triple.
    assert_eq!(
        g.triples[0].subject.description.as_deref(),
        Some("rich"),
        "triple endpoint must be normalized to the canonical merged entity"
    );
}
