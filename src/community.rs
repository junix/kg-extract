//! Community detection over an extracted [`KnowledgeGraph`] (ADR-987 §D#2).
//!
//! Closes the "orphan library" gap: `kg-community` shipped 5 clean detectors
//! behind a `CommunityDetector` trait but nothing wired the extraction output
//! into it. This adapter maps a `KnowledgeGraph` (entities keyed by id, triples
//! as edges) onto `kg_community::Graph` (contiguous node indices + edge list),
//! runs a detector, and maps the resulting `Partition` labels back to entity ids.
//!
//! Enabled by the optional `community` feature.

use std::collections::BTreeMap;

use kg_community::{CommunityDetector, Graph, LabelPropagation, Partition};

use crate::types::KnowledgeGraph;

/// Build a [`kg_community::Graph`] from a [`KnowledgeGraph`].
///
/// Entities become nodes indexed by insertion order; each triple becomes one
/// undirected edge of weight `1.0` between its subject and object. Edges are
/// **not** deduplicated: `kg_community::Graph` keeps parallel edges and the
/// detectors sum them, so N triples between the same pair of entities act as
/// an edge of weight N — KG edge multiplicity is a genuine community-strength
/// signal and MUST NOT be flattened away (that is what the old
/// `Graph::from_edges` path did). Self-loops and triples whose endpoints are
/// not registered entities are skipped. Returns the id list (index `i` ↔
/// entity id) so `Partition` labels can be mapped back.
pub fn to_community_graph(kg: &KnowledgeGraph) -> (Vec<String>, Graph) {
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    let mut ids: Vec<String> = Vec::with_capacity(kg.entities.len());
    for (id, _entity) in kg.entities.iter() {
        index.entry(id.as_str()).or_insert_with(|| {
            ids.push(id.clone());
            ids.len() - 1
        });
    }

    let mut graph = Graph::new(index.len());
    for triple in &kg.triples {
        if let (Some(&s), Some(&o)) = (
            index.get(triple.subject.id.as_str()),
            index.get(triple.object.id.as_str()),
        ) {
            graph.add_weighted_edge(s, o, 1.0);
        }
    }

    (ids, graph)
}

/// Detect communities with a caller-supplied detector, returning a stable
/// `entity_id -> community label` map.
pub fn detect_communities<D: CommunityDetector>(
    kg: &KnowledgeGraph,
    detector: &D,
) -> BTreeMap<String, usize> {
    let (ids, graph) = to_community_graph(kg);
    let partition: Partition = detector.detect(&graph);
    ids.into_iter()
        .enumerate()
        .map(|(i, id)| (id, partition.community_of(i)))
        .collect()
}

/// Convenience: detect with the dependency-free [`LabelPropagation`] detector.
pub fn detect_communities_label_propagation(kg: &KnowledgeGraph) -> BTreeMap<String, usize> {
    detect_communities(kg, &LabelPropagation::default())
}

/// Render communities as a JSON document for the CLI's `communities` output
/// format: `{num_communities, communities: {"0": [entity_id, …], …}}`.
/// Detection uses the dependency-free [`LabelPropagation`] detector over the
/// multiplicity-weighted graph (see [`to_community_graph`]). Community keys
/// ascend from `"0"`; member ids are sorted (the label map is a `BTreeMap`),
/// so the output is deterministic across runs.
pub fn communities_json(kg: &KnowledgeGraph) -> serde_json::Value {
    let labels = detect_communities_label_propagation(kg);
    let mut members: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (id, community) in labels {
        members.entry(community).or_default().push(id);
    }
    let communities: serde_json::Map<String, serde_json::Value> = members
        .into_iter()
        .map(|(c, ids)| {
            (
                c.to_string(),
                serde_json::Value::Array(ids.into_iter().map(serde_json::Value::String).collect()),
            )
        })
        .collect();
    serde_json::json!({
        "num_communities": communities.len(),
        "communities": communities,
    })
}

#[cfg(test)]
mod tests {
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
}
