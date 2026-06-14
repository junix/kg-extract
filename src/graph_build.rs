//! Shared graph-construction primitives for the extractors and the MCP store.
//!
//! These factor out logic that was copy-pasted across `simple.rs`, `schema_json.rs`,
//! `toolcall.rs` and `mcp.rs`:
//! - [`entity_id`]: the deterministic `entity_<md5(name)[..8]>` id scheme, so a
//!   graph built by any extractor (or the MCP server) is interchangeable.
//! - [`parse_entity_type`] / [`build_predicate`]: the lenient
//!   `parse → from_loose` type resolution used by the tool/MCP paths.
//! - [`GraphBuilder`]: accumulate entities (deduped by lowercased name) and
//!   resolve relationships by name (case-insensitive; dangling endpoints
//!   dropped). Type/predicate *parsing* stays at the call site so each extractor
//!   keeps its own fallback semantics (SchemaJson's strict `PhysicalObject`/`RelatedTo`
//!   fallback differs from `from_loose`).

use crate::merger::combine_entities;
use crate::types::{
    Entity, EntityType, KnowledgeGraph, MergeStrategy, Predicate, PredicateType, Triple,
};
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
    s.parse::<EntityType>()
        .unwrap_or_else(|_| EntityType::from_loose(s))
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

pub(crate) fn should_swap_passive_by(
    subject: &Entity,
    predicate: &Predicate,
    object: &Entity,
) -> bool {
    if !predicate.predicate_type.value().ends_with("_BY") {
        return false;
    }
    if is_actor_type(subject.entity_type) && !is_actor_type(object.entity_type) {
        return true;
    }
    predicate.predicate_type == PredicateType::EvidencedBy
        && looks_like_evidence_source(subject)
        && looks_like_evidenced_claim(object)
}

fn is_actor_type(t: EntityType) -> bool {
    matches!(
        t,
        EntityType::Person
            | EntityType::Organization
            | EntityType::Company
            | EntityType::Institution
            | EntityType::GovernmentAgency
            | EntityType::PoliticalParty
            | EntityType::MilitaryUnit
    )
}

fn looks_like_evidence_source(e: &Entity) -> bool {
    entity_label_contains_any(
        e,
        &[
            "audit log",
            "audit logs",
            "log",
            "logs",
            "evidence",
            "evidentiary",
            "trace",
            "traces",
            "record",
            "records",
            "metric",
            "metrics",
            "measurement",
            "measurements",
        ],
    )
}

fn looks_like_evidenced_claim(e: &Entity) -> bool {
    entity_label_contains_any(
        e,
        &[
            "incident report",
            "incident reports",
            "report",
            "reports",
            "claim",
            "claims",
            "finding",
            "findings",
            "conclusion",
            "conclusions",
            "assertion",
            "assertions",
            "hypothesis",
            "hypotheses",
        ],
    )
}

fn entity_label_contains_any(e: &Entity, needles: &[&str]) -> bool {
    let label = e.label.to_lowercase();
    needles.iter().any(|needle| label.contains(needle))
}

/// Accumulates entities and resolves relationships by name into a
/// [`KnowledgeGraph`].
///
/// Entities are deduped by lowercased name; how a same-name collision combines
/// the two is governed by [`merge_strategy`](Self::merge_strategy) (default
/// [`MergeStrategy::KeepExisting`] — first occurrence wins). Relationship
/// endpoints are resolved case-insensitively and dropped if either side is
/// unknown. The caller supplies already-parsed [`EntityType`]/[`Predicate`]
/// values, so each extractor controls its own type-fallback behaviour.
#[derive(Default)]
pub(crate) struct GraphBuilder {
    kg: KnowledgeGraph,
    by_name: HashMap<String, String>, // lowercased name -> entity id
    strategy: MergeStrategy,
}

impl GraphBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// How a same-name (lowercased) collision is combined. `KeepExisting` (the
    /// default) keeps the first occurrence; `KeepIncoming`/`FieldUnion` fold in
    /// the later one. `Llm` behaves as `FieldUnion` here (the synchronous build
    /// path makes no LLM calls; cross-segment LLM synthesis lives in the merger).
    pub(crate) fn merge_strategy(mut self, strategy: MergeStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Add an entity, deduped by lowercased name. On a collision the existing
    /// entity is combined with the incoming one per [`Self::merge_strategy`]
    /// (keeping the existing id stable). Returns the entity's id.
    pub(crate) fn add_entity_with_raw_type(
        &mut self,
        name: &str,
        entity_type: EntityType,
        raw_type: Option<String>,
        description: Option<String>,
        attributes: HashMap<String, serde_json::Value>,
    ) -> String {
        let key = name.to_lowercase();
        if let Some(id) = self.by_name.get(&key).cloned() {
            // Same entity seen again: combine per strategy. `KeepExisting` is a
            // pure no-op (the historical first-wins behaviour), so skip the work.
            if self.strategy != MergeStrategy::KeepExisting {
                if let Some(existing) = self.kg.entities.get(&id).cloned() {
                    let mut incoming = Entity::new(id.clone(), name, entity_type);
                    incoming.raw_type = raw_type;
                    incoming.description = description;
                    incoming.metadata = attributes;
                    let merged = combine_entities(self.strategy, &existing, &incoming, None);
                    self.kg.entities.insert(id.clone(), merged);
                }
            }
            return id;
        }
        let id = entity_id(name);
        let mut entity = Entity::new(id.clone(), name, entity_type);
        entity.raw_type = raw_type;
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
        let (Some(subject), Some(object)) = (
            self.kg.entities.get(&sid).cloned(),
            self.kg.entities.get(&tid).cloned(),
        ) else {
            return false;
        };
        let (subject, object) = if should_swap_passive_by(&subject, &predicate, &object) {
            (object, subject)
        } else {
            (subject, object)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passive_by_relation_swaps_actor_to_non_actor_direction() {
        let org = Entity::new("org", "Helio Systems", EntityType::Organization);
        let product = Entity::new("product", "Aurora Portal", EntityType::Product);
        let predicate = Predicate::new(PredicateType::DevelopedBy);

        assert!(should_swap_passive_by(&org, &predicate, &product));
        assert!(!should_swap_passive_by(&product, &predicate, &org));
    }

    #[test]
    fn evidenced_by_swaps_evidence_source_to_claim_direction() {
        let mut logs = Entity::new("logs", "Audit Logs", EntityType::Other);
        logs.description = Some("Operational records used as evidence.".into());
        let mut reports = Entity::new("reports", "Incident Reports", EntityType::Other);
        reports.description = Some("Reports that are evidenced by logs.".into());
        let predicate = Predicate::new(PredicateType::EvidencedBy);

        assert!(should_swap_passive_by(&logs, &predicate, &reports));
        assert!(!should_swap_passive_by(&reports, &predicate, &logs));
    }

    #[test]
    fn evidenced_by_does_not_swap_correct_direction_from_cross_descriptions() {
        let mut reports = Entity::new("reports", "Incident Reports", EntityType::Other);
        reports.description = Some("Reports evidenced by Audit Logs.".into());
        let mut logs = Entity::new("logs", "Audit Logs", EntityType::Other);
        logs.description = Some("Audit records evidencing Incident Reports.".into());
        let predicate = Predicate::new(PredicateType::EvidencedBy);

        assert!(!should_swap_passive_by(&reports, &predicate, &logs));
    }
}
