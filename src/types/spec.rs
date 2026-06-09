//! The declarative extraction **spec** — the "what" half of the spec/execution
//! split.
//!
//! An [`ExtractionSpec`] bundles the target schema, how strictly to honour it
//! ([`SchemaMode`]), and the output dedup policy. It is orthogonal to
//! *execution* (model, chunking, segmentation), which lives in
//! [`ExtractionConfig`](super::ExtractionConfig) alongside an embedded spec. The
//! spec is reusable across executors (run the same spec through Youtu or
//! ToolCall) and serializable as a portable artifact.

use super::schema::Schema;
use serde::{Deserialize, Serialize};

/// How a seed schema constrains the schema-driven extractors (Youtu, ToolCall).
///
/// These are three of the four cells of the (schema present?) × (may add types?)
/// grid; the fourth — constrain to an *empty* schema — is meaningless and is made
/// unrepresentable by requiring a non-empty schema for `Fixed`/`Evolving`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchemaMode {
    /// No predefined schema; the model infers entity/relation types freely.
    #[default]
    Open,
    /// Closed: use only the types in the seeded schema. Requires a non-empty schema.
    Fixed,
    /// Seeded schema, but the model may propose new types (`new_schema_types`).
    /// Requires a non-empty schema (an empty seed is just `Open`).
    Evolving,
}

impl SchemaMode {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            SchemaMode::Open => "open",
            SchemaMode::Fixed => "fixed",
            SchemaMode::Evolving => "evolving",
        }
    }

    /// `Fixed`/`Evolving` extract against a seed schema; `Open` does not.
    pub(crate) fn needs_schema(&self) -> bool {
        matches!(self, SchemaMode::Fixed | SchemaMode::Evolving)
    }
}

/// The declarative contract for an extraction: *what* graph shape is wanted,
/// independent of *how* it is produced.
///
/// This is the "spec" half of the spec/execution split — reusable across
/// executors and serializable. The "execution" half (model, chunking,
/// segmentation) lives in [`ExtractionConfig`](super::ExtractionConfig), which
/// embeds one of these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractionSpec {
    /// Target ontology: entity types, relation types, attributes.
    #[serde(default)]
    pub schema: Schema,
    /// How strictly the schema constrains extraction.
    #[serde(default)]
    pub mode: SchemaMode,
    /// Dedup entities (by lowercased label) and triples in the output graph.
    #[serde(default = "default_true")]
    pub merge_duplicates: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ExtractionSpec {
    fn default() -> Self {
        ExtractionSpec { schema: Schema::default(), mode: SchemaMode::Open, merge_duplicates: true }
    }
}

impl ExtractionSpec {
    /// An open spec with no seeded schema (the default).
    pub fn open() -> Self {
        Self::default()
    }

    /// A spec seeded with `schema`, in `mode`. (`Fixed`/`Evolving` need a
    /// non-empty schema; the extractor validates this at `extract` time.)
    pub fn new(schema: Schema, mode: SchemaMode) -> Self {
        ExtractionSpec { schema, mode, merge_duplicates: true }
    }
}
