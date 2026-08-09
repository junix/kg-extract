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

    // --- entity_id: the deterministic shared id scheme ----------------------

    #[test]
    fn entity_id_is_entity_prefix_plus_first_eight_md5_hex() {
        // Pinned to the documented `entity_<md5(name)[..8]>` scheme so a graph
        // built by any extractor (or the MCP store) stays interchangeable.
        // Changing the digest or prefix silently breaks every stored id.
        assert_eq!(entity_id("OpenAI"), "entity_0523b132");
        assert_eq!(entity_id("GPT-4"), "entity_f7559929");
        assert_eq!(entity_id("Alice"), "entity_64489c85");
        // Format invariant: lowercase hex, exactly 8 chars after the prefix.
        let id = entity_id("arbitrary name");
        assert!(id.starts_with("entity_"));
        assert_eq!(id.len(), "entity_".len() + 8);
        assert!(id["entity_".len()..].chars().all(|c| c.is_ascii_hexdigit()));
        // Keyed on raw bytes (case-sensitive), as documented.
        assert_ne!(entity_id("Alice"), entity_id("alice"));
        // Deterministic across calls.
        assert_eq!(entity_id("OpenAI"), entity_id("OpenAI"));
    }

    // --- parse_entity_type / build_predicate: the tool/MCP type resolution --

    #[test]
    fn parse_entity_type_empty_or_unknown_falls_back_to_other() {
        assert_eq!(parse_entity_type(""), EntityType::Other);
        assert_eq!(parse_entity_type("   "), EntityType::Other, "input is trimmed first");
        assert_eq!(
            parse_entity_type("totally unknown"),
            EntityType::Other,
            "unknown tokens fall back via from_loose"
        );
    }

    #[test]
    fn parse_entity_type_exact_parse_then_loose_alias() {
        // Exact SCREAMING_SNAKE parse wins first.
        assert_eq!(parse_entity_type("PERSON"), EntityType::Person);
        assert_eq!(parse_entity_type("  ORGANIZATION  "), EntityType::Organization);
        // When the exact parse fails, from_loose aliasing resolves it.
        assert_eq!(parse_entity_type("Person"), EntityType::Person);
    }

    #[test]
    fn build_predicate_normalises_separators_and_keeps_raw_label() {
        // Spaces/dashes -> underscores, upper-cased, then parsed.
        let p = build_predicate("developed by");
        assert_eq!(p.predicate_type, PredicateType::DevelopedBy);
        assert_eq!(p.label.as_deref(), Some("developed by"));
        assert_eq!(p.raw_type.as_deref(), Some("developed by"));
        assert_eq!(p.output_type(), "developed by", "raw label wins over the normalised value");

        // Already-canonical upper form parses directly.
        assert_eq!(
            build_predicate("DEVELOPED_BY").predicate_type,
            PredicateType::DevelopedBy
        );
        // Dash separator normalises the same way.
        assert_eq!(
            build_predicate("is-used-by").predicate_type,
            PredicateType::IsUsedBy
        );
        // Unknown relation falls back to RelatedTo but still keeps the raw label.
        let unk = build_predicate("utterly fabricated relation");
        assert_eq!(unk.predicate_type, PredicateType::RelatedTo);
        assert_eq!(unk.label.as_deref(), Some("utterly fabricated relation"));
    }

    // --- GraphBuilder: dedup, dangling-drop, merge strategy, attributes -----

    #[test]
    fn graph_builder_dedups_entities_case_insensitively_with_stable_id() {
        let mut b = GraphBuilder::new();
        let id1 = b.add_entity_with_raw_type(
            "OpenAI",
            EntityType::Organization,
            None,
            None,
            HashMap::new(),
        );
        // A name differing only by case collides and returns the SAME id; the
        // shared md5 scheme means the id is the one computed from the first name.
        let id2 = b.add_entity_with_raw_type(
            "openai",
            EntityType::Company,
            None,
            None,
            HashMap::new(),
        );
        assert_eq!(id1, id2);
        assert_eq!(id1, entity_id("OpenAI"));
        let g = b.into_graph();
        assert_eq!(g.entities.len(), 1, "case-variant names collapse to one entity");
        // KeepExisting (the default): first occurrence wins, the later one is discarded.
        let e = g.entities.values().next().unwrap();
        assert_eq!(e.entity_type, EntityType::Organization);
        assert_eq!(e.label, "OpenAI");
    }

    #[test]
    fn graph_builder_add_relation_drops_dangling_and_resolves_case_insensitively() {
        let mut b = GraphBuilder::new();
        b.add_entity_with_raw_type("A", EntityType::Other, None, None, HashMap::new());
        // Dangling target -> nothing added, returns false.
        let added = b.add_relation(
            "A",
            Predicate::new(PredicateType::Uses),
            "Ghost",
            |_| {},
        );
        assert!(!added, "a relation to an unknown endpoint must be dropped");
        // Case-insensitive name resolution: "a" resolves to the entity added as "A".
        let added2 = b.add_relation(
            "a",
            Predicate::new(PredicateType::Uses),
            "A",
            |_| {},
        );
        assert!(added2);
        let g = b.into_graph();
        assert_eq!(
            g.triples.len(),
            1,
            "only the resolved relation survives; the dangling one is absent"
        );
    }

    #[test]
    fn graph_builder_field_union_keeps_richer_description_and_specific_type() {
        let mut b = GraphBuilder::new().merge_strategy(MergeStrategy::FieldUnion);
        b.add_entity_with_raw_type(
            "Acme",
            EntityType::Organization,
            None,
            Some("short".into()),
            HashMap::new(),
        );
        b.add_entity_with_raw_type(
            "acme",
            EntityType::Other,
            None,
            Some("a much longer, richer description".into()),
            HashMap::new(),
        );
        let g = b.into_graph();
        assert_eq!(g.entities.len(), 1);
        let e = g.entities.values().next().unwrap();
        assert_eq!(e.entity_type, EntityType::Organization, "specific type beats Other");
        assert_eq!(
            e.description.as_deref(),
            Some("a much longer, richer description"),
            "FieldUnion keeps the richer description"
        );
        // Canonical (first-occurrence) id survives the merge.
        assert_eq!(e.id, entity_id("Acme"));
    }

    #[test]
    fn graph_builder_set_attribute_is_noop_on_unknown_case_insensitive_on_known() {
        // Unknown entity: no-op (no panic, no entity created).
        let mut b = GraphBuilder::new();
        b.set_attribute("Ghost", "k".into(), serde_json::json!(1));
        assert!(b.into_graph().entities.is_empty());

        // Known entity, looked up case-insensitively.
        let mut b = GraphBuilder::new();
        b.add_entity_with_raw_type("Node", EntityType::Other, None, None, HashMap::new());
        b.set_attribute("node", "score".into(), serde_json::json!(7));
        let g = b.into_graph();
        let e = g.entities.values().next().unwrap();
        assert_eq!(e.metadata["score"], serde_json::json!(7));
    }
}
