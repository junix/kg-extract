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
/// undirected edge between its subject and object. Self-loops and triples whose
/// endpoints are not registered entities are skipped. Returns the id list
/// (index `i` ↔ entity id) so `Partition` labels can be mapped back.
pub fn to_community_graph(kg: &KnowledgeGraph) -> (Vec<String>, Graph) {
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    let mut ids: Vec<String> = Vec::with_capacity(kg.entities.len());
    for (id, _entity) in kg.entities.iter() {
        index.entry(id.as_str()).or_insert_with(|| {
            ids.push(id.clone());
            ids.len() - 1
        });
    }

    let mut edges: Vec<(usize, usize)> = Vec::with_capacity(kg.triples.len());
    for triple in &kg.triples {
        if let (Some(&s), Some(&o)) = (
            index.get(triple.subject.id.as_str()),
            index.get(triple.object.id.as_str()),
        ) {
            if s != o {
                edges.push((s, o));
            }
        }
    }

    (ids, Graph::from_edges(index.len(), &edges))
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
