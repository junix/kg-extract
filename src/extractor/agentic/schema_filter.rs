//! Closed-world schema validation for the agentic extractor.
//!
//! Resolves a [`SchemaMode`](crate::types::SchemaMode) + [`Schema`] into a
//! [`SchemaPolicy`], then applies that policy to each slice's parse:
//! - [`SchemaPolicy::Fixed`] drops out-of-schema records (and feeds back so the
//!   model self-corrects on the next turn).
//! - [`SchemaPolicy::Evolving`] keeps everything but records the types used
//!   outside the seed as proposals.
//! - [`SchemaPolicy::Off`] leaves extraction unconstrained.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::types::{Entity, Schema, SchemaMode, Triple};

/// Normalize a type token to the canonical comparison form used for schema
/// matching: trimmed, uppercased, spaces/dashes → underscores.
pub(super) fn norm_type(s: &str) -> String {
    s.trim().to_uppercase().replace([' ', '-'], "_")
}

/// What one slice's validation against a fixed schema produced.
pub(super) struct SliceFilter {
    pub kept_entities: HashMap<String, Entity>,
    pub kept_triples: Vec<Triple>,
    pub dropped_types: BTreeSet<String>,
    pub dropped_records: usize,
}

impl SliceFilter {
    /// The slice yielded records but none survived the schema — the degenerate
    /// case that warrants a bounded re-do.
    pub(super) fn all_dropped(&self) -> bool {
        self.dropped_records > 0 && self.kept_entities.is_empty() && self.kept_triples.is_empty()
    }
}

/// Closed-world validator for [`SchemaMode::Fixed`]: the allowed entity and
/// relation types, normalized for matching. An empty half (no node or no
/// relation constraint) lets everything through that half.
pub(super) struct SchemaFilter {
    nodes: HashSet<String>,
    relations: HashSet<String>,
}

impl SchemaFilter {
    /// Build the seed-type sets, or `None` if the schema is empty (nothing to
    /// enforce or evolve from — the degenerate `Fixed`/`Evolving` cell that is
    /// just `Open`).
    pub(super) fn build(schema: &Schema) -> Option<Self> {
        let nodes: HashSet<String> = schema.nodes.iter().map(|s| norm_type(s)).collect();
        let relations: HashSet<String> = schema.relations.iter().map(|s| norm_type(s)).collect();
        if nodes.is_empty() && relations.is_empty() {
            None
        } else {
            Some(SchemaFilter { nodes, relations })
        }
    }

    /// Collect the normalized entity/relation types that appear in a parse but
    /// are *outside* the seed schema — the `Evolving`-mode proposals. A seed half
    /// left empty is treated as unconstrained (nothing is "new" for it), mirroring
    /// the filter semantics.
    pub(super) fn new_types(
        &self,
        entities: &HashMap<String, Entity>,
        triples: &[Triple],
        type_tokens: &HashMap<String, String>,
    ) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut nodes = BTreeSet::new();
        let mut relations = BTreeSet::new();
        for e in entities.values() {
            let raw = type_tokens
                .get(&e.label.trim().to_lowercase())
                .cloned()
                .unwrap_or_else(|| e.entity_type.to_string());
            let t = norm_type(&raw);
            if !self.node_ok(&t) {
                nodes.insert(t);
            }
        }
        for tr in triples {
            let rl = tr
                .predicate
                .label
                .clone()
                .unwrap_or_else(|| tr.predicate.predicate_type.to_string());
            let r = norm_type(&rl);
            if !self.rel_ok(&r) {
                relations.insert(r);
            }
        }
        (nodes, relations)
    }

    pub(super) fn node_ok(&self, t: &str) -> bool {
        self.nodes.is_empty() || self.nodes.contains(t)
    }

    pub(super) fn rel_ok(&self, t: &str) -> bool {
        self.relations.is_empty() || self.relations.contains(t)
    }

    /// Partition a slice's parse: drop entities whose type is out-of-schema and
    /// relations whose type is out-of-schema or whose endpoint was dropped.
    /// `type_tokens` carries each entity's *raw* type string (see
    /// [`crate::extractor::simple::entity_type_tokens`]) so domain-specific
    /// schema types — ones outside the known [`crate::types::EntityType`]
    /// vocabulary that the enum would collapse to `Other` — still match.
    pub(super) fn apply(
        &self,
        entities: HashMap<String, Entity>,
        triples: Vec<Triple>,
        type_tokens: &HashMap<String, String>,
    ) -> SliceFilter {
        let mut kept_entities = HashMap::new();
        let mut dropped_types = BTreeSet::new();
        let mut dropped_records = 0usize;

        for (id, e) in entities {
            let raw = type_tokens
                .get(&e.label.trim().to_lowercase())
                .cloned()
                .unwrap_or_else(|| e.entity_type.to_string());
            let t = norm_type(&raw);
            if self.node_ok(&t) {
                kept_entities.insert(id, e);
            } else {
                dropped_records += 1;
                dropped_types.insert(t);
            }
        }

        let mut kept_triples = Vec::new();
        for tr in triples {
            let endpoints_ok = kept_entities.contains_key(&tr.subject.id)
                && kept_entities.contains_key(&tr.object.id);
            let rel_label = tr
                .predicate
                .label
                .clone()
                .unwrap_or_else(|| tr.predicate.predicate_type.to_string());
            let rel = norm_type(&rel_label);
            let rel_ok = self.rel_ok(&rel);
            if endpoints_ok && rel_ok {
                kept_triples.push(tr);
            } else {
                dropped_records += 1;
                if !rel_ok {
                    dropped_types.insert(rel);
                }
            }
        }

        SliceFilter {
            kept_entities,
            kept_triples,
            dropped_types,
            dropped_records,
        }
    }
}

/// How the seed schema governs a run, resolved from [`SchemaMode`] + schema.
pub(super) enum SchemaPolicy {
    /// Unconstrained: `Open`, or `Fixed`/`Evolving` with an empty schema.
    Off,
    /// Closed-world: validate each slice, drop out-of-schema, feed back.
    Fixed(SchemaFilter),
    /// Seeded but open: keep everything, record types outside the seed.
    Evolving(SchemaFilter),
}

impl SchemaPolicy {
    pub(super) fn for_mode(mode: SchemaMode, schema: &Schema) -> Self {
        match mode {
            SchemaMode::Open => SchemaPolicy::Off,
            SchemaMode::Fixed => SchemaFilter::build(schema)
                .map(SchemaPolicy::Fixed)
                .unwrap_or(SchemaPolicy::Off),
            SchemaMode::Evolving => SchemaFilter::build(schema)
                .map(SchemaPolicy::Evolving)
                .unwrap_or(SchemaPolicy::Off),
        }
    }
}
