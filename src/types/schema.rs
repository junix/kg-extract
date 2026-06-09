//! Knowledge graph schema definition (ported from `graph/_types/schema.py`).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

/// Knowledge graph schema: the entity types, relation types and attributes that
/// guide extraction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    #[serde(default)]
    pub nodes: Vec<String>,
    #[serde(default)]
    pub relations: Vec<String>,
    #[serde(default)]
    pub attributes: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Schema {
    pub fn new(nodes: Vec<String>, relations: Vec<String>, attributes: Vec<String>) -> Self {
        Schema { nodes, relations, attributes, metadata: HashMap::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.relations.is_empty() && self.attributes.is_empty()
    }

    /// Load from a JSON file. Accepts both `Nodes/Relations/Attributes`
    /// (capitalised, SchemaJson style) and lowercase keys.
    pub fn from_json_file(path: impl AsRef<Path>) -> anyhow::Result<Schema> {
        let data = std::fs::read_to_string(path)?;
        Self::from_json_str(&data)
    }

    pub fn from_json_str(data: &str) -> anyhow::Result<Schema> {
        let v: serde_json::Value = serde_json::from_str(data)?;
        let get_list = |cap: &str, low: &str| -> Vec<String> {
            v.get(cap)
                .or_else(|| v.get(low))
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                .unwrap_or_default()
        };
        let metadata = v
            .get("metadata")
            .and_then(|m| m.as_object())
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        Ok(Schema {
            nodes: get_list("Nodes", "nodes"),
            relations: get_list("Relations", "relations"),
            attributes: get_list("Attributes", "attributes"),
            metadata,
        })
    }

    /// Formatted context string for prompt inclusion.
    pub fn to_prompt_context(&self) -> String {
        format!(
            "Entity Types: {}\nRelation Types: {}\nAttribute Types: {}",
            self.nodes.join(", "),
            self.relations.join(", "),
            self.attributes.join(", ")
        )
    }

    /// Merge with another schema, set-unioning each list.
    pub fn merge(&self, other: &Schema) -> Schema {
        let union = |a: &[String], b: &[String]| -> Vec<String> {
            a.iter().chain(b.iter()).cloned().collect::<BTreeSet<_>>().into_iter().collect()
        };
        let mut metadata = self.metadata.clone();
        metadata.extend(other.metadata.clone());
        Schema {
            nodes: union(&self.nodes, &other.nodes),
            relations: union(&self.relations, &other.relations),
            attributes: union(&self.attributes, &other.attributes),
            metadata,
        }
    }
}
