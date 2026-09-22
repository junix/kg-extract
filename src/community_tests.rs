use super::*;
use crate::types::{Entity, EntityType, Predicate, PredicateType, Triple};

fn triple(subj: &str, obj: &str) -> Triple {
    Triple::new(
        Entity::new(subj, subj, EntityType::Other),
        Predicate::new(PredicateType::RelatedTo),
        Entity::new(obj, obj, EntityType::Other),
    )
}

#[test]
fn maps_entities_and_triples_onto_a_community_graph() {
    let mut kg = KnowledgeGraph::new();
    // Two disjoint triangles: {a,b,c} and {x,y,z}.
    for (s, o) in [
        ("a", "b"),
        ("b", "c"),
        ("c", "a"),
        ("x", "y"),
        ("y", "z"),
        ("z", "x"),
    ] {
        kg.add_triple(triple(s, o));
    }
    let (ids, graph) = to_community_graph(&kg);
    assert_eq!(ids.len(), 6);
    assert_eq!(graph.num_nodes(), 6);
    assert_eq!(graph.edges().len(), 6);
}

#[test]
fn multiplicity_produces_parallel_weighted_edges() {
    // Three triples between the same pair must NOT collapse into one edge:
    // kg-community sums parallel edges, so multiplicity becomes weight.
    let mut kg = KnowledgeGraph::new();
    for _ in 0..3 {
        kg.add_triple(triple("a", "b"));
    }
    kg.add_triple(triple("b", "c"));

    let (ids, graph) = to_community_graph(&kg);
    let pos = |id: &str| ids.iter().position(|i| i == id).unwrap();
    let (a, b, c) = (pos("a"), pos("b"), pos("c"));
    assert_eq!(graph.edges().len(), 4, "every triple keeps its own edge");
    let weight_ab: f64 = graph
        .edges()
        .iter()
        .filter(|&&(s, o, _)| (s == a && o == b) || (s == b && o == a))
        .map(|&(_, _, w)| w)
        .sum();
    assert_eq!(weight_ab, 3.0, "multiplicity is preserved as summed weight");
    let weight_bc: f64 = graph
        .edges()
        .iter()
        .filter(|&&(s, o, _)| (s == b && o == c) || (s == c && o == b))
        .map(|&(_, _, w)| w)
        .sum();
    assert_eq!(weight_bc, 1.0);
}

#[test]
fn canonical_direction_merges_direction_variant_multiplicity() {
    // (a, USES, b) and (b, IS_USED_BY, a) are the same semantic edge in two
    // directions. After canonical-direction normalisation + merge dedup
    // they are ONE triple, so the community graph sees one edge of weight
    // 1.0 — while a genuinely distinct edge (a, PART_OF, b) keeps its own
    // weight, so the pair's summed multiplicity lands at 2.0 (not 3.0).
    use crate::merger::{merge_with_deduplication, normalize_direction};

    let ent = |id: &str| Entity::new(id, id, EntityType::Other);
    let directed =
        |s: &str, p: PredicateType, o: &str| Triple::new(ent(s), Predicate::new(p), ent(o));

    let mut g1 = KnowledgeGraph::new();
    g1.add_triple(directed("a", PredicateType::Uses, "b"));
    let mut g2 = KnowledgeGraph::new();
    g2.add_triple(directed("b", PredicateType::IsUsedBy, "a"));
    g2.add_triple(directed("a", PredicateType::PartOf, "b"));
    normalize_direction(&mut g1);
    normalize_direction(&mut g2);

    let kg = merge_with_deduplication(g1, g2);
    assert_eq!(
        kg.triples.len(),
        2,
        "direction variants collapse to one edge"
    );

    let (ids, graph) = to_community_graph(&kg);
    let pos = |id: &str| ids.iter().position(|i| i == id).unwrap();
    let (a, b) = (pos("a"), pos("b"));
    assert_eq!(graph.edges().len(), 2);
    let weight_ab: f64 = graph
        .edges()
        .iter()
        .filter(|&&(s, o, _)| (s == a && o == b) || (s == b && o == a))
        .map(|&(_, _, w)| w)
        .sum();
    assert_eq!(
        weight_ab, 2.0,
        "merged variant (1.0) + distinct PART_OF edge (1.0), not 3.0"
    );
}

#[test]
fn multiplicity_weights_change_the_partition() {
    // n1–n2 is a heavy pair (5 triples); n0–n1, n2–n3, n0–n3 are single
    // triples. Summed multiplicity must bind n1 to n2 (and n0 to n3),
    // yielding TWO communities; the old dedup adapter (every edge weight
    // 1.0) collapses the whole component into one.
    let mut kg = KnowledgeGraph::new();
    kg.add_triple(triple("n0", "n1"));
    kg.add_triple(triple("n2", "n3"));
    for _ in 0..5 {
        kg.add_triple(triple("n1", "n2"));
    }
    kg.add_triple(triple("n0", "n3"));
    let labels = detect_communities_label_propagation(&kg);
    assert_eq!(labels["n1"], labels["n2"], "the heavy pair binds");
    assert_eq!(labels["n0"], labels["n3"]);
    assert_ne!(
        labels["n0"], labels["n1"],
        "multiplicity splits the component where a flat graph would not"
    );
}

#[test]
fn communities_json_groups_members_by_community() {
    let mut kg = KnowledgeGraph::new();
    for (s, o) in [
        ("a", "b"),
        ("b", "c"),
        ("c", "a"),
        ("x", "y"),
        ("y", "z"),
        ("z", "x"),
    ] {
        kg.add_triple(triple(s, o));
    }
    let value = communities_json(&kg);
    assert_eq!(value["num_communities"], 2);
    assert!(
        value["quality"].is_null(),
        "label propagation reports no quality score"
    );
    let communities = value["communities"].as_object().unwrap();
    assert_eq!(communities.len(), 2);
    let mut groups: Vec<Vec<String>> = communities
        .values()
        .map(|v| {
            v.as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap().to_string())
                .collect()
        })
        .collect();
    for g in &mut groups {
        g.sort();
    }
    groups.sort();
    assert_eq!(
        groups,
        vec![
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec!["x".to_string(), "y".to_string(), "z".to_string()],
        ]
    );
}

#[test]
fn detects_two_disjoint_clusters() {
    let mut kg = KnowledgeGraph::new();
    for (s, o) in [
        ("a", "b"),
        ("b", "c"),
        ("c", "a"),
        ("x", "y"),
        ("y", "z"),
        ("z", "x"),
    ] {
        kg.add_triple(triple(s, o));
    }
    let labels = detect_communities_label_propagation(&kg);
    assert_eq!(labels.len(), 6);
    // Same triangle → same community; different triangles → different.
    assert_eq!(labels["a"], labels["b"]);
    assert_eq!(labels["b"], labels["c"]);
    assert_eq!(labels["x"], labels["y"]);
    assert_ne!(labels["a"], labels["x"]);
}
