//! Predicate type + the `Predicate` model.
//!
//! `PredicateType` and its resolution logic now live in the shared **kg-vocab**
//! crate (the single source of truth for the 108-variant predicate vocabulary)
//! and are re-exported here so callers keep using `crate::types::PredicateType`.
//! See ADR-987 §D#5.
//!
//! kg-vocab v2+ (`kg.vocab.v3` pinned) additionally exposes
//! `PredicateType::inverse()`
//! (converse predicate for an edge flip; unpaired predicates invert to
//! themselves) and the `ENTITY_GROUPS` / `PREDICATE_GROUPS` tables through the
//! same re-export — no extra wiring needed.
//! TODO(kg-vocab v2): `inverse()` could normalise relationship direction at
//! merge time (`merger.rs` dedups triples by `(subj, predicate, obj)`), e.g.
//! folding `A -IS_USED_BY-> B` into `B -USES-> A`; left undone because it
//! changes dedup semantics and needs a spec decision first.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use kg_vocab::{default_predicates, PredicateType};

/// Represents a relationship predicate between entities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Predicate {
    pub predicate_type: PredicateType,
    /// Original relation type token emitted by the model/tool before enum
    /// normalisation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Predicate {
    pub fn new(predicate_type: PredicateType) -> Self {
        Predicate {
            predicate_type,
            raw_type: None,
            label: None,
            confidence: None,
            metadata: HashMap::new(),
        }
    }

    pub fn with_label(predicate_type: PredicateType, label: impl Into<String>) -> Self {
        let label = label.into();
        Predicate {
            predicate_type,
            raw_type: Some(label.clone()),
            label: Some(label),
            confidence: None,
            metadata: HashMap::new(),
        }
    }

    pub fn output_type(&self) -> String {
        self.raw_type
            .as_deref()
            .or(self.label.as_deref())
            .filter(|s| !s.trim().is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| self.predicate_type.value())
    }

    /// Human-readable label: `label` if set, else Title-Cased type value.
    pub fn display_label(&self) -> String {
        if let Some(l) = &self.label {
            return l.clone();
        }
        if let Some(raw) = &self.raw_type {
            return raw.clone();
        }
        title_case(&self.predicate_type.value())
    }
}

/// Convert `SOME_VALUE` → `Some Value` (mirrors Python `.replace("_"," ").title()`).
fn title_case(screaming: &str) -> String {
    screaming
        .split('_')
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c
                    .to_uppercase()
                    .chain(chars.flat_map(|c| c.to_lowercase()))
                    .collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    // Behavior-preservation guard: PredicateType now comes from kg-vocab.
    use super::*;
    use kg_vocab::TypeMatch;

    #[test]
    fn roundtrip_values() {
        assert_eq!(PredicateType::IsA.value(), "IS_A");
        assert_eq!(PredicateType::WorksFor.value(), "WORKS_FOR");
        assert_eq!(
            PredicateType::ReducesDimensionality.value(),
            "REDUCES_DIMENSIONALITY"
        );
    }

    #[test]
    fn loose_match() {
        assert_eq!(PredicateType::from_loose("uses"), PredicateType::Uses);
        assert_eq!(
            PredicateType::from_loose("is used by"),
            PredicateType::IsUsedBy
        );
        assert_eq!(
            PredicateType::from_loose("no such thing"),
            PredicateType::RelatedTo
        );
    }

    #[test]
    fn resolve_reports_match_kind() {
        assert_eq!(
            PredicateType::resolve("uses"),
            (PredicateType::Uses, TypeMatch::Exact)
        );
        assert_eq!(
            PredicateType::resolve("is developed by"),
            (PredicateType::DevelopedBy, TypeMatch::Aliased)
        );
        assert_eq!(
            PredicateType::resolve("no such thing"),
            (PredicateType::RelatedTo, TypeMatch::Fallback)
        );
        assert_eq!(
            PredicateType::resolve(""),
            (PredicateType::RelatedTo, TypeMatch::Fallback)
        );
    }

    #[test]
    fn loose_empty_relation_is_related_to() {
        assert_eq!(PredicateType::from_loose(""), PredicateType::RelatedTo);
        assert_eq!(PredicateType::from_loose("   "), PredicateType::RelatedTo);
    }

    #[test]
    fn display_label_titlecase() {
        assert_eq!(
            Predicate::new(PredicateType::DevelopedBy).display_label(),
            "Developed By"
        );
    }

    #[test]
    fn variant_count() {
        assert_eq!(PredicateType::all().len(), 108);
        assert_eq!(default_predicates().len(), 108);
    }

    #[test]
    fn kg_vocab_v3_parse_semantics() {
        // kg-vocab v2 (`kg.vocab.v2`) intentionally changed loose predicate
        // parsing, and v3 (`kg.vocab.v3`) refined it again — these assertions
        // pin the current upstream behaviour:
        assert_eq!(kg_vocab::VOCAB_VERSION, "kg.vocab.v3");
        // Inputs normalising to <3 chars fall back without fuzzy matching
        // (v1 aliased "in" to LOCATED_IN).
        assert_eq!(
            PredicateType::resolve("in"),
            (PredicateType::RelatedTo, TypeMatch::Fallback)
        );
        // Longest variant wins on substring matches (v1 resolved "used" to
        // USED_IN; both USED_IN and IS_USED_BY contain it at a `_` boundary).
        assert_eq!(PredicateType::from_loose("used"), PredicateType::IsUsedBy);
        // Substring matching requires a `_` word boundary: "overfit" is a
        // bare stem of OVERFITS (no boundary after "OVERFIT"), so it falls
        // back instead of aliasing.
        assert_eq!(
            PredicateType::resolve("overfit"),
            (PredicateType::RelatedTo, TypeMatch::Fallback)
        );
        // v3: the curated disambiguation table wins before the fuzzy scan.
        // "tested"/"validated" intentionally CHANGE v2's declaration-order
        // results (TESTED_BY/VALIDATED_BY) to the _ON variants (ML corpus:
        // "tested/validated on <benchmark>" dominates); "invented"/
        // "published" pin v2's results (INVENTED_BY/PUBLISHED_IN).
        assert_eq!(
            PredicateType::resolve("tested"),
            (PredicateType::TestedOn, TypeMatch::Aliased)
        );
        assert_eq!(
            PredicateType::resolve("validated"),
            (PredicateType::ValidatedOn, TypeMatch::Aliased)
        );
        assert_eq!(PredicateType::from_loose("invented"), PredicateType::InventedBy);
        assert_eq!(PredicateType::from_loose("published"), PredicateType::PublishedIn);
        // v3: an equal-length tie among the longest substring matches is NOT
        // broken by declaration order — it falls back to RELATED_TO unless
        // the disambiguation table pins it ("tested by tested on" matches
        // TESTED_BY and TESTED_ON at equal length, and "TESTED BY TESTED ON"
        // itself is not a curated key).
        assert_eq!(
            PredicateType::resolve("tested by tested on"),
            (PredicateType::RelatedTo, TypeMatch::Fallback)
        );
    }

    #[test]
    fn kg_vocab_v3_inverse_and_groups() {
        // Curated inverse pairs from vocab.json; unpaired predicates invert
        // to themselves (total-function semantics).
        assert_eq!(PredicateType::Uses.inverse(), PredicateType::IsUsedBy);
        assert_eq!(PredicateType::IsUsedBy.inverse(), PredicateType::Uses);
        assert_eq!(PredicateType::RelatedTo.inverse(), PredicateType::RelatedTo);
        // Group tables ship for domain-scoped schema trimming; not yet
        // consumed here (see module-level TODO).
        assert!(!kg_vocab::PREDICATE_GROUPS.is_empty());
        assert!(!kg_vocab::ENTITY_GROUPS.is_empty());
    }
}
