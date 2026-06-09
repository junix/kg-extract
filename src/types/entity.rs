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
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString, EnumIter, AsRefStr,
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

impl EntityType {
    /// The SCREAMING_SNAKE_CASE string value (matches Python `EntityType.value`).
    pub fn value(&self) -> String {
        self.to_string()
    }

    /// Parse a free-form type string into an [`EntityType`], applying the same
    /// normalisation and fallback mapping used by the Python parser:
    /// `upper().replace(" ", "_")`, then exact match, then common aliases,
    /// then `OTHER`.
    pub fn from_loose(raw: &str) -> EntityType {
        let normalized = raw.trim().to_uppercase().replace([' ', '-'], "_");
        if let Ok(t) = normalized.parse::<EntityType>() {
            return t;
        }
        match normalized.as_str() {
            "MODEL" => EntityType::Technology,
            "ALGORITHM" => EntityType::Technology,
            "DATASET" => EntityType::Technology,
            "METHOD" => EntityType::Technology,
            "METRIC" => EntityType::Category,
            "RESEARCH_GROUP" => EntityType::Organization,
            "GPU_CLUSTER" => EntityType::Technology,
            "REWARD_FUNCTION" => EntityType::Technology,
            "OPTIMIZATION_ALGORITHM" => EntityType::Technology,
            "FINETUNING_METHOD" => EntityType::Technology,
            _ => EntityType::Other,
        }
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
            confidence: None,
            description: None,
            metadata: HashMap::new(),
        }
    }

    /// Dict form matching Python `Entity.to_dict` (`type` key holds the value).
    pub fn to_dict(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "label": self.label,
            "type": self.entity_type.value(),
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
        assert_eq!(EntityType::from_loose("model"), EntityType::Technology);
        assert_eq!(EntityType::from_loose("totally unknown"), EntityType::Other);
        assert_eq!(EntityType::from_loose("Person"), EntityType::Person);
    }

    #[test]
    fn variant_count() {
        assert_eq!(EntityType::all().len(), 122);
    }
}
