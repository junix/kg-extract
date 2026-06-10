//! Predicate / relationship type definitions for knowledge graph extraction.
//!
//! Ported from `graph/_types/predicates.py`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use strum::IntoEnumIterator;
use strum_macros::{AsRefStr, Display, EnumIter, EnumString};

/// Enumeration of all supported predicate types for relationship extraction.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    Display,
    EnumString,
    EnumIter,
    AsRefStr,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum PredicateType {
    // Geographic & Location
    Population,
    Area,
    LocatedIn,
    OriginatesFrom,
    OccursIn,
    // Usage & Application
    UsedIn,
    IsUsedBy,
    Uses,
    AppliesTo,
    AppliedIn,
    // Improvement & Enablement
    Improves,
    Enables,
    Optimizes,
    // Development & Creation
    DevelopedBy,
    InventedBy,
    InventedIn,
    DiscoveredIn,
    FormulatedBy,
    // Effects & Causation
    Affects,
    Causes,
    Prevents,
    Inhibits,
    Catalyzes,
    TransformsInto,
    // Structural Relations
    PartOf,
    Includes,
    Excludes,
    ComposedOf,
    BelongsTo,
    // Organization & People
    FoundedBy,
    WorksFor,
    SpecializesIn,
    CollaboratesWith,
    TeachesAt,
    // Research & Academia
    PublishedIn,
    PublishedBy,
    Researches,
    FundedBy,
    Discovers,
    PresentedAt,
    PeerReviewedBy,
    CitedBy,
    ConductedBy,
    ReportedIn,
    // Dependencies & Requirements
    Requires,
    DerivesFrom,
    DerivedFrom,
    // Production & Consumption
    Produces,
    Consumes,
    // Representation & Symbolism
    Symbolizes,
    Represents,
    Signifies,
    Indicates,
    // Influence & Interaction
    Attracts,
    Influences,
    InteractsWith,
    CompetesWith,
    AssociatedWith,
    RelatedTo,
    // Ontological Relations
    IsA,
    HasProperty,
    // Contribution & Support
    ContributesTo,
    SupportedBy,
    // Governance & Regulation
    Governs,
    Regulates,
    ApprovedBy,
    // Preservation & Destruction
    Preserves,
    Destroys,
    Protects,
    // Temporal Relations
    Succeeds,
    Precedes,
    // Logical Relations
    Contradicts,
    Complements,
    // Validation & Testing
    ValidatedBy,
    EvaluatedBy,
    TestedBy,
    MeasuredBy,
    DetectedBy,
    EvidencedBy,
    // Analysis & Modeling
    AnalyzedBy,
    ModelledBy,
    SimulatedBy,
    InferredFrom,
    ExaminedIn,
    // Implementation & Deployment
    ImplementedIn,
    DeployedIn,
    IntegratedWith,
    ModifiedBy,
    // Machine Learning Relations
    TrainedOn,
    ValidatedOn,
    TestedOn,
    TunedBy,
    Predicts,
    Classifies,
    Regresses,
    Clusters,
    ReducesDimensionality,
    Performs,
    Generalizes,
    Overfits,
    Underfits,
    Regularizes,
    Converges,
    Diverges,
    Initializes,
    // Medical & Treatment
    Treats,
    Celebrates,
    Measures,
}

impl PredicateType {
    /// The SCREAMING_SNAKE_CASE string value (matches Python `PredicateType.value`).
    pub fn value(&self) -> String {
        self.to_string()
    }

    /// All variants (mirrors `get_default_predicates`).
    pub fn all() -> Vec<PredicateType> {
        PredicateType::iter().collect()
    }

    /// Map a free-form relation string to a [`PredicateType`], mirroring the
    /// Python parser: normalise (`upper().replace(" ", "_")`), exact match,
    /// then substring match either direction, else `RELATED_TO`.
    pub fn from_loose(raw: &str) -> PredicateType {
        let normalized = raw.trim().to_uppercase().replace([' ', '-'], "_");
        // A blank predicate must not fuzzy-match: `value.contains("")` is always
        // true, so the substring loop below would return the first variant.
        if normalized.is_empty() {
            return PredicateType::RelatedTo;
        }
        if let Ok(p) = normalized.parse::<PredicateType>() {
            return p;
        }
        for pt in PredicateType::iter() {
            let v = pt.value();
            if normalized.contains(&v) || v.contains(&normalized) {
                return pt;
            }
        }
        PredicateType::RelatedTo
    }
}

/// Default list of predicate types for extraction.
pub fn default_predicates() -> Vec<PredicateType> {
    PredicateType::all()
}

/// Represents a relationship predicate between entities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Predicate {
    pub predicate_type: PredicateType,
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
            label: None,
            confidence: None,
            metadata: HashMap::new(),
        }
    }

    pub fn with_label(predicate_type: PredicateType, label: impl Into<String>) -> Self {
        Predicate {
            predicate_type,
            label: Some(label.into()),
            confidence: None,
            metadata: HashMap::new(),
        }
    }

    /// Human-readable label: `label` if set, else Title-Cased type value.
    pub fn display_label(&self) -> String {
        if let Some(l) = &self.label {
            return l.clone();
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
    use super::*;

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
    fn loose_empty_relation_is_related_to() {
        // An empty / whitespace-only relation must not fuzzy-match the first
        // variant: `"POPULATION".contains("")` is always true, which previously
        // returned `Population` for any blank predicate.
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
    }
}
