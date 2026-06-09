//! Shared graph-construction primitives for the extractors and the MCP store.
//!
//! These factor out logic that was copy-pasted across `simple.rs`, `youtu.rs`,
//! `toolcall.rs` and `mcp.rs`:
//! - [`entity_id`]: the deterministic `entity_<md5(name)[..8]>` id scheme, so a
//!   graph built by any extractor (or the MCP server) is interchangeable.
//! - [`parse_entity_type`] / [`build_predicate`]: the lenient
//!   `parse → from_loose` type resolution used by the tool/MCP paths.
//! - [`GraphBuilder`]: accumulate entities (deduped by lowercased name) and
//!   resolve relationships by name (case-insensitive; dangling endpoints
//!   dropped). Type/predicate *parsing* stays at the call site so each extractor
//!   keeps its own fallback semantics (Youtu's strict `PhysicalObject`/`RelatedTo`
//!   fallback differs from `from_loose`).

use crate::types::{Entity, EntityType, KnowledgeGraph, Predicate, PredicateType, Triple};
use std::collections::HashMap;

/// Deterministic id for an entity name: `entity_<md5(name)[..8]>`.
///
/// Shared by all extractors and the MCP store so their outputs are
/// interchangeable. Keyed on the raw name bytes (case-sensitive), matching the
/// Python original.
pub(crate) fn entity_id(name: &str) -> String {
    let digest = format!("{:x}", md5::compute(name.as_bytes()));
    format!("entity_{}", &digest[..8])
}

/// Resolve a free-form type string to an [`EntityType`]: exact parse first, then
/// the lenient [`EntityType::from_loose`] aliasing; an empty string is treated as
/// `"other"`. Used by the tool-call and MCP paths.
pub(crate) fn parse_entity_type(s: &str) -> EntityType {
    let s = s.trim();
    if s.is_empty() {
        return EntityType::from_loose("other");
    }
    s.parse::<EntityType>().unwrap_or_else(|_| EntityType::from_loose(s))
}

/// Build a [`Predicate`] from a free-form relation string, keeping the raw string
/// as the display label. Normalises (`upper`, `' '`/`'-'` → `'_'`), parses, then
/// falls back to the lenient [`PredicateType::from_loose`].
pub(crate) fn build_predicate(s: &str) -> Predicate {
    let pt = s
        .to_uppercase()
        .replace([' ', '-'], "_")
        .parse::<PredicateType>()
        .unwrap_or_else(|_| PredicateType::from_loose(s));
    Predicate::with_label(pt, s.to_string())
}

/// Accumulates entities and resolves relationships by name into a
/// [`KnowledgeGraph`].
///
/// Entities are deduped by lowercased name (first occurrence wins); relationship
/// endpoints are resolved case-insensitively and dropped if either side is
/// unknown. The caller supplies already-parsed [`EntityType`]/[`Predicate`]
/// values, so each extractor controls its own type-fallback behaviour.
#[derive(Default)]
pub(crate) struct GraphBuilder {
    kg: KnowledgeGraph,
    by_name: HashMap<String, String>, // lowercased name -> entity id
}

impl GraphBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Add an entity, deduped by lowercased name (first occurrence wins).
    /// Returns the entity's id (existing or freshly minted).
    pub(crate) fn add_entity(
        &mut self,
        name: &str,
        entity_type: EntityType,
        description: Option<String>,
        attributes: HashMap<String, serde_json::Value>,
    ) -> String {
        let key = name.to_lowercase();
        if let Some(id) = self.by_name.get(&key) {
            return id.clone();
        }
        let id = entity_id(name);
        let mut entity = Entity::new(id.clone(), name, entity_type);
        entity.description = description;
        entity.metadata = attributes;
        self.by_name.insert(key, id.clone());
        self.kg.add_entity(entity);
        id
    }

    /// Resolve `source`/`target` by name (case-insensitive) and add a triple,
    /// running `decorate` on it first (e.g. to set confidence/description).
    /// Returns `false` and adds nothing if either endpoint is unknown.
    pub(crate) fn add_relation(
        &mut self,
        source: &str,
        predicate: Predicate,
        target: &str,
        decorate: impl FnOnce(&mut Triple),
    ) -> bool {
        let sid = self.by_name.get(&source.to_lowercase()).cloned();
        let tid = self.by_name.get(&target.to_lowercase()).cloned();
        let (Some(sid), Some(tid)) = (sid, tid) else {
            return false;
        };
        let (Some(subject), Some(object)) =
            (self.kg.entities.get(&sid).cloned(), self.kg.entities.get(&tid).cloned())
        else {
            return false;
        };
        let mut triple = Triple::new(subject, predicate, object);
        decorate(&mut triple);
        self.kg.add_triple(triple);
        true
    }

    /// Set an attribute on a previously-added entity (by name). No-op if unknown.
    /// Call after [`add_relation`]s, since `add_triple` re-inserts endpoint
    /// entities and would otherwise clobber the enriched copy.
    pub(crate) fn set_attribute(&mut self, name: &str, key: String, value: serde_json::Value) {
        if let Some(id) = self.by_name.get(&name.to_lowercase()) {
            if let Some(e) = self.kg.entities.get_mut(id) {
                e.metadata.insert(key, value);
            }
        }
    }

    /// Consume the builder, yielding the accumulated graph.
    pub(crate) fn into_graph(self) -> KnowledgeGraph {
        self.kg
    }
}
