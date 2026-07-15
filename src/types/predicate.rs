//! Predicate type + the `Predicate` model.
//!
//! `PredicateType` and its resolution logic now live in the shared **kg-vocab**
//! crate (the single source of truth for the 108-variant predicate vocabulary)
//! and are re-exported here so callers keep using `crate::types::PredicateType`.
//! See ADR-987 §D#5.

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
}
