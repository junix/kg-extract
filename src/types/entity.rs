//! Entity type definitions for knowledge graph extraction.
//!
//! Ported from `graph/_types/entities.py`. Variant string values are the
//! SCREAMING_SNAKE_CASE form (e.g. `EntityType::Person` ⇄ `"PERSON"`),
//! produced consistently by both serde and strum.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use strum::IntoEnumIterator;
use strum_macros::{AsRefStr, Display, EnumIter, EnumString};

/// Enumeration of all supported entity types for NER and knowledge extraction.
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
pub enum EntityType {
    // Astronomical & Physical
    CelestialBody,
    Planet,
    AstronomicalObject,
    SpaceMission,
    // Environmental & Natural
    Ecosystem,
    NaturalDisaster,
    NaturalPhenomenon,
    EnvironmentalPhenomenon,
    GeologicalFormation,
    // Technology & Engineering
    Weapon,
    Vehicle,
    Technology,
    Software,
    Product,
    Material,
    DigitalAsset,
    TechnicalStandard,
    // Architecture & Geography
    ArchitecturalStyle,
    Landmark,
    Location,
    City,
    Country,
    // Culture & Society
    Cuisine,
    DanceForm,
    EthnicGroup,
    SocialMovement,
    CulturalConcept,
    CulturalHeritage,
    Holiday,
    // People & Organizations
    Person,
    Organization,
    Company,
    Institution,
    GovernmentAgency,
    PoliticalParty,
    MilitaryUnit,
    // Fiction & Mythology
    FictionalCharacter,
    MythologicalFigure,
    // Concepts & Philosophy
    PhilosophicalConcept,
    PsychologicalTrait,
    Theory,
    Ideology,
    // Legal & Political
    Treaty,
    Law,
    // Scientific & Academic
    ScientificTerm,
    AcademicField,
    ScientificDiscipline,
    UnitOfMeasurement,
    Measurement,
    PhysicalConstant,
    PhysicalLaw,
    // Medical & Biological
    MedicalCondition,
    Disease,
    ChemicalSubstance,
    ChemicalElement,
    Nutrient,
    Species,
    BiologicalSpecimen,
    BiologicalProcess,
    ChemicalReaction,
    Protein,
    Molecule,
    CellType,
    GeneticMarker,
    // Arts & Media
    WorkOfArt,
    ArtMovement,
    MusicalGenre,
    LiteraryGenre,
    // Sports & Games
    Sport,
    Game,
    // Time & Events
    Date,
    Time,
    Event,
    HistoricalPeriod,
    HistoricalEvent,
    // Language & Religion
    Language,
    Religion,
    // Economics
    Currency,
    // Awards & Recognition
    Award,
    // Physical Objects
    PhysicalObject,
    Mineral,
    // Mathematics & Computing
    Algorithm,
    MathematicalModel,
    Theorem,
    Hypothesis,
    Variable,
    Model,
    // Machine Learning
    NeuralNetwork,
    Layer,
    Neuron,
    Feature,
    Hyperparameter,
    LossFunction,
    Optimizer,
    ActivationFunction,
    TrainingSet,
    ValidationSet,
    TestSet,
    CrossValidation,
    Epoch,
    BatchSize,
    LearningRate,
    Regularization,
    // Research & Academia
    Experiment,
    ResearchPaper,
    Grant,
    Dataset,
    Conference,
    Journal,
    ResearchMethod,
    StatisticalMethod,
    ClinicalTrial,
    Survey,
    // Programming
    ComputerProgram,
    // Physics
    QuantumState,
    QuantumParticle,
    // Virtual
    VirtualEntity,
    // Generic
    Position,
    Number,
    Occupation,
    Category,
    Other,
}

/// How a free-form type token resolved against the canonical vocabulary.
///
/// Returned by [`EntityType::resolve`] and [`PredicateType::resolve`] so a graph
/// can report which tokens were remapped or lost rather than silently degrading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeMatch {
    /// The token is an exact canonical variant — no information lost.
    Exact,
    /// The token was remapped to a related canonical type via the alias table.
    Aliased,
    /// The token is out of vocabulary and fell back to the generic catch-all
    /// (`OTHER` / `RELATED_TO`); the raw token is the only record of its meaning.
    Fallback,
}

impl EntityType {
    /// The SCREAMING_SNAKE_CASE string value (matches Python `EntityType.value`).
    pub fn value(&self) -> String {
        self.to_string()
    }

    /// Parse a free-form type string into an [`EntityType`], applying the same
    /// normalisation and fallback mapping used by the Python parser:
    /// `upper().replace(" ", "_")`, then exact match, then common aliases,
    /// then `OTHER`. Discards the resolution provenance; use [`resolve`] when you
    /// need to know whether the token aliased or fell back.
    ///
    /// [`resolve`]: EntityType::resolve
    pub fn from_loose(raw: &str) -> EntityType {
        Self::resolve(raw).0
    }

    /// Like [`from_loose`], but also reports *how* the token resolved
    /// ([`TypeMatch::Exact`] / [`Aliased`] / [`Fallback`]) so callers can audit
    /// out-of-vocabulary tokens instead of silently collapsing them to `OTHER`.
    ///
    /// [`from_loose`]: EntityType::from_loose
    /// [`Aliased`]: TypeMatch::Aliased
    /// [`Fallback`]: TypeMatch::Fallback
    pub fn resolve(raw: &str) -> (EntityType, TypeMatch) {
        let normalized = raw.trim().to_uppercase().replace([' ', '-'], "_");
        if let Ok(t) = normalized.parse::<EntityType>() {
            return (t, TypeMatch::Exact);
        }
        match Self::alias(&normalized) {
            Some(t) => (t, TypeMatch::Aliased),
            None => (EntityType::Other, TypeMatch::Fallback),
        }
    }

    /// Curated alias table: maps common out-of-vocabulary type tokens to the
    /// closest canonical variant *before* the `OTHER` fallback. Only consulted
    /// when no exact variant matches, so it can never override a real type.
    ///
    /// Extends the Python original (which had only the first block) with
    /// software/ML/research tokens that models routinely emit; every added entry
    /// previously collapsed to `OTHER`, so this strictly recovers information.
    fn alias(normalized: &str) -> Option<EntityType> {
        use EntityType::*;
        Some(match normalized {
            // --- Python-parity block (preserved verbatim) ---
            "MODEL" => Technology,
            "ALGORITHM" => Technology,
            "DATASET" => Technology,
            "METHOD" => Technology,
            "METRIC" => Category,
            "RESEARCH_GROUP" => Organization,
            "GPU_CLUSTER" => Technology,
            "REWARD_FUNCTION" => Technology,
            "OPTIMIZATION_ALGORITHM" => Technology,
            "FINETUNING_METHOD" => Technology,
            // --- Software / AI artefacts → Technology ---
            "AI_MODEL"
            | "ML_MODEL"
            | "LLM"
            | "LANGUAGE_MODEL"
            | "LARGE_LANGUAGE_MODEL"
            | "FOUNDATION_MODEL" => Technology,
            "FRAMEWORK" | "LIBRARY" | "API" | "SDK" | "TOOLKIT" | "PROTOCOL" | "PLATFORM"
            | "TOOL" | "SERVICE" | "APPLICATION" | "APP" | "SYSTEM" => Technology,
            // --- Data artefacts → Dataset ---
            "BENCHMARK" | "CORPUS" | "TRAINING_DATA" => Dataset,
            // --- Commercial orgs → Company / Institution / Organization ---
            "STARTUP" | "CORPORATION" | "VENDOR" => Company,
            "UNIVERSITY" | "LABORATORY" | "LAB" | "RESEARCH_LAB" | "RESEARCH_INSTITUTE" => {
                Institution
            }
            "TEAM" | "RESEARCH_TEAM" | "GROUP" | "CONSORTIUM" => Organization,
            // --- Human roles → Person ---
            "RESEARCHER" | "SCIENTIST" | "ENGINEER" | "AUTHOR" | "DEVELOPER" | "FOUNDER"
            | "PROFESSOR" => Person,
            _ => return None,
        })
    }

    /// All variants (mirrors `get_default_entity_types`).
    pub fn all() -> Vec<EntityType> {
        EntityType::iter().collect()
    }
}

/// Default list of entity types for extraction.
pub fn default_entity_types() -> Vec<EntityType> {
    EntityType::all()
}

/// Represents an extracted entity with its type and metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub label: String,
    pub entity_type: EntityType,
    /// Original type token emitted by the model/tool before enum normalisation.
    ///
    /// This keeps open-schema extraction lossless while `entity_type` remains
    /// available for legacy enum-based code paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Entity {
    pub fn new(id: impl Into<String>, label: impl Into<String>, entity_type: EntityType) -> Self {
        Entity {
            id: id.into(),
            label: label.into(),
            entity_type,
            raw_type: None,
            confidence: None,
            description: None,
            metadata: HashMap::new(),
        }
    }

    pub fn with_raw_type(mut self, raw_type: impl Into<String>) -> Self {
        self.raw_type = Some(raw_type.into());
        self
    }

    /// The type token to expose in open-schema output.
    pub fn output_type(&self) -> String {
        self.raw_type
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| self.entity_type.value())
    }

    /// Dict form matching Python `Entity.to_dict` (`type` key holds the value).
    pub fn to_dict(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "label": self.label,
            "type": self.output_type(),
            "normalized_type": self.entity_type.value(),
            "confidence": self.confidence,
            "description": self.description,
            "metadata": self.metadata,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_values() {
        assert_eq!(EntityType::Person.value(), "PERSON");
        assert_eq!(EntityType::CelestialBody.value(), "CELESTIAL_BODY");
        assert_eq!(EntityType::BatchSize.value(), "BATCH_SIZE");
        assert_eq!("PERSON".parse::<EntityType>().unwrap(), EntityType::Person);
    }

    #[test]
    fn loose_parse_fallbacks() {
        // Exact match wins (MODEL is a real variant), mirroring Python EntityType("MODEL").
        assert_eq!(EntityType::from_loose("model"), EntityType::Model);
        // Alias path only fires when there is no exact variant (METHOD is not one).
        assert_eq!(EntityType::from_loose("method"), EntityType::Technology);
        assert_eq!(
            EntityType::from_loose("research group"),
            EntityType::Organization
        );
        assert_eq!(EntityType::from_loose("totally unknown"), EntityType::Other);
        assert_eq!(EntityType::from_loose("Person"), EntityType::Person);
    }

    #[test]
    fn resolve_reports_match_kind() {
        // Exact canonical variant.
        assert_eq!(
            EntityType::resolve("person"),
            (EntityType::Person, TypeMatch::Exact)
        );
        // Extended alias table recovers tokens that previously hit OTHER.
        assert_eq!(
            EntityType::resolve("LLM"),
            (EntityType::Technology, TypeMatch::Aliased)
        );
        assert_eq!(
            EntityType::resolve("research lab"),
            (EntityType::Institution, TypeMatch::Aliased)
        );
        assert_eq!(
            EntityType::resolve("researcher"),
            (EntityType::Person, TypeMatch::Aliased)
        );
        // Genuinely unknown still falls back, but now it is reported as such.
        assert_eq!(
            EntityType::resolve("totally unknown"),
            (EntityType::Other, TypeMatch::Fallback)
        );
    }

    #[test]
    fn variant_count() {
        assert_eq!(EntityType::all().len(), 122);
    }
}
