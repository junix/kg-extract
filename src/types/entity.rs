//! Entity type + the extracted `Entity` model.
//!
//! `EntityType` / `TypeMatch` and the alias/resolution logic now live in the
//! shared **kg-vocab** crate (the single source of truth for the 122-variant
//! entity vocabulary), and are re-exported here so callers keep using
//! `crate::types::EntityType`. Variant string values are still the
//! SCREAMING_SNAKE_CASE form (`EntityType::Person` ⇄ `"PERSON"`). See ADR-987 §D#5.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use kg_vocab::{default_entity_types, EntityType, TypeMatch};

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
#[path = "entity_tests.rs"]
mod tests;

