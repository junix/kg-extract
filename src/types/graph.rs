//! Core graph data structures (ported from `graph/_types/graph.py`).

use super::entity::Entity;
use super::predicate::{Predicate, PredicateType};
use indexmap_lite::OrderedMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A knowledge graph triple (subject-predicate-object).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Triple {
    pub subject: Entity,
    pub predicate: Predicate,
    pub object: Entity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Triple {
    pub fn new(subject: Entity, predicate: Predicate, object: Entity) -> Self {
        Triple {
            subject,
            predicate,
            object,
            confidence: None,
            metadata: HashMap::new(),
        }
    }

    /// `(subject_id, predicate_type_value, object_id)` — the dedup key.
    pub fn to_tuple(&self) -> (String, String, String) {
        (
            self.subject.id.clone(),
            self.predicate.predicate_type.value(),
            self.object.id.clone(),
        )
    }

    pub fn to_dict(&self) -> serde_json::Value {
        serde_json::json!({
            "subject": self.subject.to_dict(),
            "predicate": {
                "type": self.predicate.predicate_type.value(),
                "label": self.predicate.label,
                "confidence": self.predicate.confidence,
                "metadata": self.predicate.metadata,
            },
            "object": self.object.to_dict(),
            "confidence": self.confidence,
            "metadata": self.metadata,
        })
    }
}

/// A complete knowledge graph: entities keyed by id (insertion-ordered) + triples.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KnowledgeGraph {
    pub entities: OrderedMap<String, Entity>,
    pub triples: Vec<Triple>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl KnowledgeGraph {
    pub fn new() -> Self {
        KnowledgeGraph::default()
    }

    pub fn add_entity(&mut self, entity: Entity) {
        self.entities.insert(entity.id.clone(), entity);
    }

    pub fn add_triple(&mut self, triple: Triple) {
        self.upsert_endpoint(triple.subject.clone());
        self.upsert_endpoint(triple.object.clone());
        self.triples.push(triple);
    }

    /// Insert a triple endpoint, keeping the historical replace semantics but
    /// never losing provenance: citations already recorded on the stored
    /// entity are unioned into the incoming snapshot before it overwrites.
    fn upsert_endpoint(&mut self, entity: Entity) {
        #[allow(unused_mut)]
        let mut entity = entity;
        #[cfg(feature = "citations")]
        if let Some(existing) = self.entities.get(&entity.id) {
            crate::citation::union_citations(&mut entity.metadata, &existing.metadata);
        }
        self.add_entity(entity);
    }

    pub fn get_entity(&self, id: &str) -> Option<&Entity> {
        self.entities.get(id)
    }

    pub fn get_triples_by_predicate(&self, pt: PredicateType) -> Vec<&Triple> {
        self.triples
            .iter()
            .filter(|t| t.predicate.predicate_type == pt)
            .collect()
    }

    /// Merge another graph into `self` (mirrors `KnowledgeGraph.merge`):
    /// entities added if absent (or replaced when higher confidence), triples
    /// deduplicated by `to_tuple`.
    pub fn merge(&mut self, other: KnowledgeGraph) -> &mut Self {
        for (_, entity) in other.entities.iter() {
            match self.entities.get(&entity.id) {
                None => self.add_entity(entity.clone()),
                Some(existing) => {
                    let replace = match (entity.confidence, existing.confidence) {
                        (Some(new_c), Some(old_c)) => new_c > old_c,
                        (Some(_), None) => true,
                        _ => false,
                    };
                    if replace {
                        self.entities.insert(entity.id.clone(), entity.clone());
                    }
                }
            }
        }
        let mut existing: HashSet<(String, String, String)> =
            self.triples.iter().map(|t| t.to_tuple()).collect();
        for triple in other.triples {
            // `insert` returns false when the key was already present, which
            // dedups identical triples *within* `other` as well as against
            // `self`. Push directly rather than via `add_triple`: the endpoints
            // were already entity-merged above, and re-inserting the triple's
            // (possibly staler, lower-confidence) endpoint snapshots would
            // clobber that merge. Only materialise an endpoint that is missing.
            if existing.insert(triple.to_tuple()) {
                if !self.entities.contains_key(&triple.subject.id) {
                    self.add_entity(triple.subject.clone());
                }
                if !self.entities.contains_key(&triple.object.id) {
                    self.add_entity(triple.object.clone());
                }
                // Bind the triple to the canonical (merged) entity snapshots so a
                // triple endpoint can never disagree with the entity table — the
                // entity-merge above may have kept a richer/higher-confidence copy
                // than the one embedded in this triple.
                let subject = self
                    .entities
                    .get(&triple.subject.id)
                    .cloned()
                    .unwrap_or_else(|| triple.subject.clone());
                let object = self
                    .entities
                    .get(&triple.object.id)
                    .cloned()
                    .unwrap_or_else(|| triple.object.clone());
                let Triple {
                    predicate,
                    confidence,
                    metadata,
                    ..
                } = triple;
                self.triples.push(Triple {
                    subject,
                    object,
                    predicate,
                    confidence,
                    metadata,
                });
            }
        }
        self
    }

    pub fn to_dict(&self) -> serde_json::Value {
        let entities: serde_json::Map<String, serde_json::Value> = self
            .entities
            .iter()
            .map(|(k, v)| (k.clone(), v.to_dict()))
            .collect();
        serde_json::json!({
            "entities": entities,
            "triples": self.triples.iter().map(|t| t.to_dict()).collect::<Vec<_>>(),
            "metadata": self.metadata,
        })
    }

    /// Node-link JSON (the D3 / NetworkX `node_link_data` interchange shape): a
    /// `nodes` array plus a `links` array whose entries reference node `id`s via
    /// `source`/`target`. Understood by D3 force layouts, NetworkX, and most
    /// graph-viz tools.
    ///
    /// This differs from [`to_dict`], which keeps the RDF-style
    /// `{entities, triples: [{subject, predicate, object}]}` shape. Here each
    /// triple becomes a link whose `subject.id` → `source` and
    /// `object.id` → `target`.
    ///
    /// [`to_dict`]: KnowledgeGraph::to_dict
    pub fn to_node_link(&self) -> serde_json::Value {
        let nodes: Vec<serde_json::Value> =
            self.entities.iter().map(|(_, e)| e.to_dict()).collect();
        let links: Vec<serde_json::Value> = self
            .triples
            .iter()
            .map(|t| {
                serde_json::json!({
                    "source": t.subject.id,
                    "target": t.object.id,
                    "type": t.predicate.predicate_type.value(),
                    "label": t.predicate.label,
                    "confidence": t.confidence,
                    "metadata": t.metadata,
                })
            })
            .collect();
        serde_json::json!({
            "directed": true,
            "multigraph": false,
            "graph": self.metadata,
            "nodes": nodes,
            "links": links,
        })
    }

    /// Mermaid `graph LR` diagram (ported from `to_mermaid`).
    pub fn to_mermaid(&self) -> String {
        let clean = |s: &str| s.replace(['[', ']'], "");
        let mut lines = vec!["graph LR".to_string()];
        for (id, entity) in self.entities.iter() {
            lines.push(format!("    {}[{}]", clean(id), entity.label));
        }
        for t in &self.triples {
            lines.push(format!(
                "    {} -->| {} | {}",
                clean(&t.subject.id),
                t.predicate.display_label(),
                clean(&t.object.id)
            ));
        }
        lines.push(String::new());
        lines.push("    %% Styling".to_string());
        lines.push("    classDef default fill:none,stroke:#ffffff,color:#ffffff".to_string());
        lines.push("    linkStyle default stroke:#EAEDED,stroke-width:2px".to_string());
        lines.join("\n")
    }

    pub fn stats(&self) -> serde_json::Value {
        let mut entity_types: HashMap<String, usize> = HashMap::new();
        for (_, e) in self.entities.iter() {
            *entity_types.entry(e.entity_type.value()).or_insert(0) += 1;
        }
        let mut predicate_types: HashMap<String, usize> = HashMap::new();
        for t in &self.triples {
            *predicate_types
                .entry(t.predicate.predicate_type.value())
                .or_insert(0) += 1;
        }
        serde_json::json!({
            "num_entities": self.entities.len(),
            "num_triples": self.triples.len(),
            "entity_types": entity_types,
            "predicate_types": predicate_types,
        })
    }
}

#[cfg(test)]
mod tests {
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
}

/// Minimal insertion-ordered map (we avoid an `indexmap` dependency; entity
/// order matters for stable mermaid / merge output).
pub mod indexmap_lite {
    use serde::de::{Deserializer, MapAccess, Visitor};
    use serde::ser::{SerializeMap, Serializer};
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;
    use std::hash::Hash;
    use std::marker::PhantomData;

    #[derive(Debug, Clone)]
    pub struct OrderedMap<K: Eq + Hash + Clone, V> {
        order: Vec<K>,
        map: HashMap<K, V>,
    }

    impl<K: Eq + Hash + Clone, V> Default for OrderedMap<K, V> {
        fn default() -> Self {
            OrderedMap {
                order: Vec::new(),
                map: HashMap::new(),
            }
        }
    }

    impl<K: Eq + Hash + Clone, V> OrderedMap<K, V> {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn insert(&mut self, k: K, v: V) {
            if !self.map.contains_key(&k) {
                self.order.push(k.clone());
            }
            self.map.insert(k, v);
        }
        pub fn get<Q>(&self, k: &Q) -> Option<&V>
        where
            K: std::borrow::Borrow<Q>,
            Q: Eq + Hash + ?Sized,
        {
            self.map.get(k)
        }
        pub fn get_mut<Q>(&mut self, k: &Q) -> Option<&mut V>
        where
            K: std::borrow::Borrow<Q>,
            Q: Eq + Hash + ?Sized,
        {
            self.map.get_mut(k)
        }
        pub fn contains_key<Q>(&self, k: &Q) -> bool
        where
            K: std::borrow::Borrow<Q>,
            Q: Eq + Hash + ?Sized,
        {
            self.map.contains_key(k)
        }
        pub fn len(&self) -> usize {
            self.order.len()
        }
        pub fn is_empty(&self) -> bool {
            self.order.is_empty()
        }
        pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
            self.order
                .iter()
                .map(move |k| (k, self.map.get(k).unwrap()))
        }
        pub fn values(&self) -> impl Iterator<Item = &V> {
            self.order.iter().map(move |k| self.map.get(k).unwrap())
        }
        /// Mutable access to every value (iteration order unspecified).
        pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
            self.map.values_mut()
        }
        pub fn keys(&self) -> impl Iterator<Item = &K> {
            self.order.iter()
        }
    }

    impl<K, Q, V> std::ops::Index<&Q> for OrderedMap<K, V>
    where
        K: Eq + Hash + Clone + std::borrow::Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        type Output = V;
        fn index(&self, k: &Q) -> &V {
            self.map.get(k).expect("no entry found for key")
        }
    }

    impl<K, V> Serialize for OrderedMap<K, V>
    where
        K: Eq + Hash + Clone + Serialize,
        V: Serialize,
    {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            let mut m = s.serialize_map(Some(self.order.len()))?;
            for k in &self.order {
                m.serialize_entry(k, self.map.get(k).unwrap())?;
            }
            m.end()
        }
    }

    impl<'de, K, V> Deserialize<'de> for OrderedMap<K, V>
    where
        K: Eq + Hash + Clone + Deserialize<'de>,
        V: Deserialize<'de>,
    {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V2<K, V>(PhantomData<(K, V)>);
            impl<'de, K, V> Visitor<'de> for V2<K, V>
            where
                K: Eq + Hash + Clone + Deserialize<'de>,
                V: Deserialize<'de>,
            {
                type Value = OrderedMap<K, V>;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("a map")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
                    let mut om = OrderedMap::new();
                    while let Some((k, v)) = a.next_entry::<K, V>()? {
                        om.insert(k, v);
                    }
                    Ok(om)
                }
            }
            d.deserialize_map(V2(PhantomData))
        }
    }
}
