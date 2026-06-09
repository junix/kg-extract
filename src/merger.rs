//! Knowledge-graph merging utilities (ported from `graph/kg_extractor/merger.py`).

use crate::types::{Entity, KnowledgeGraph, Triple};
use regex::Regex;
use std::collections::HashSet;

/// Merge `graph2` into `graph1`. When `merge_duplicates`, entities are
/// deduplicated by lowercased label; otherwise a plain id-based merge is used.
pub fn merge_knowledge_graphs(
    mut graph1: KnowledgeGraph,
    graph2: KnowledgeGraph,
    merge_duplicates: bool,
) -> KnowledgeGraph {
    if merge_duplicates {
        merge_with_deduplication(graph1, graph2)
    } else {
        graph1.merge(graph2);
        graph1
    }
}

/// Merge two graphs deduplicating entities by `label.lower()`, remapping the
/// second graph's triples and dropping duplicate triples.
pub fn merge_with_deduplication(mut g1: KnowledgeGraph, g2: KnowledgeGraph) -> KnowledgeGraph {
    // label(lower) -> id, seeded from g1.
    let mut label_to_id = std::collections::HashMap::new();
    for (id, e) in g1.entities.iter() {
        label_to_id.insert(e.label.to_lowercase(), id.clone());
    }

    // id in g2 -> id in merged g1.
    let mut id_mapping = std::collections::HashMap::new();

    for (id, entity) in g2.entities.iter() {
        let label_lower = entity.label.to_lowercase();
        if let Some(existing_id) = label_to_id.get(&label_lower) {
            id_mapping.insert(id.clone(), existing_id.clone());
        } else if g1.entities.contains_key(id) {
            // ID collision with a different label → fresh id. The entity's own
            // `.id` field must be rewritten to the new key, otherwise the stored
            // entity (and every triple endpoint cloned from it below) keeps the
            // colliding old id — leaving the table key and entity.id disagreeing
            // and the remapped triple pointing at the *other* entity's id.
            let new_id = generate_new_id(&g1);
            let mut entity = entity.clone();
            entity.id = new_id.clone();
            id_mapping.insert(id.clone(), new_id.clone());
            g1.entities.insert(new_id.clone(), entity);
            label_to_id.insert(label_lower, new_id);
        } else {
            g1.entities.insert(id.clone(), entity.clone());
            label_to_id.insert(label_lower, id.clone());
            id_mapping.insert(id.clone(), id.clone());
        }
    }

    let mut existing: HashSet<(String, String, String)> =
        g1.triples.iter().map(|t| t.to_tuple()).collect();

    for triple in g2.triples {
        let subj_id = id_mapping.get(&triple.subject.id).cloned().unwrap_or(triple.subject.id.clone());
        let obj_id = id_mapping.get(&triple.object.id).cloned().unwrap_or(triple.object.id.clone());
        let (Some(subj), Some(obj)) = (g1.entities.get(&subj_id).cloned(), g1.entities.get(&obj_id).cloned())
        else {
            continue;
        };
        let remapped = Triple {
            subject: subj,
            predicate: triple.predicate,
            object: obj,
            confidence: triple.confidence,
            metadata: triple.metadata,
        };
        let key = remapped.to_tuple();
        if !existing.contains(&key) {
            existing.insert(key);
            g1.triples.push(remapped);
        }
    }

    g1
}

/// Merge a list of graphs left-to-right with triple dedup (mirrors
/// `BaseExtractor.merge_knowledge_graphs`).
pub fn merge_all(graphs: Vec<KnowledgeGraph>) -> KnowledgeGraph {
    let mut iter = graphs.into_iter();
    let Some(mut acc) = iter.next() else {
        return KnowledgeGraph::new();
    };
    for g in iter {
        acc.merge(g);
    }
    acc
}

/// Generate `[N+1]`, where N is the max integer found in any existing id.
pub fn generate_new_id(graph: &KnowledgeGraph) -> String {
    let re = Regex::new(r"\d+").unwrap();
    let mut max_id = 0i64;
    for id in graph.entities.keys() {
        if let Some(m) = re.find(id) {
            if let Ok(n) = m.as_str().parse::<i64>() {
                max_id = max_id.max(n);
            }
        }
    }
    format!("[{}]", max_id + 1)
}

/// Find entities with an identical (case-insensitive) label.
pub fn find_similar_entities(entity: &Entity, entities: &[Entity]) -> Vec<String> {
    let target = entity.label.to_lowercase();
    entities
        .iter()
        .filter(|e| e.id != entity.id && e.label.to_lowercase() == target)
        .map(|e| e.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
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
        g2.add_triple(Triple::new(bob.clone(), Predicate::new(PredicateType::LocatedIn), paris));

        let merged = merge_with_deduplication(g1, g2);

        // Table key and entity.id must agree for every entity.
        for (key, e) in merged.entities.iter() {
            assert_eq!(&e.id, key, "entity '{}' stored under key '{}' has stale id", e.label, key);
        }
        let bob_key = merged
            .entities
            .iter()
            .find(|(_, e)| e.label == "Bob")
            .map(|(k, _)| k.clone())
            .expect("Bob present");
        assert_ne!(bob_key, "e1", "Bob must get a fresh id, not Alice's e1");
        // The Bob->Paris triple endpoint must carry Bob's fresh id.
        let t = merged.triples.iter().find(|t| t.subject.label == "Bob").expect("Bob relation kept");
        assert_eq!(t.subject.id, bob_key, "triple subject id must match Bob's table key");
    }
}
