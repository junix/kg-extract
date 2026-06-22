//! The declarative extraction **spec** — the "what" half of the spec/execution
//! split.
//!
//! An [`ExtractionSpec`] bundles the target schema, how strictly to honour it
//! ([`SchemaMode`]), and the output dedup policy. It is orthogonal to
//! *execution* (model, chunking, segmentation), which lives in
//! [`ExtractionConfig`](super::ExtractionConfig) alongside an embedded spec. The
//! spec is reusable across executors (run the same spec through SchemaJson or
//! ToolCall) and serializable as a portable artifact.

use super::schema::Schema;
use serde::{Deserialize, Serialize};

/// How a seed schema constrains the schema-driven extractors (SchemaJson, ToolCall).
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

/// How two entities (or triples) judged to be the *same* — same lowercased
/// label — are combined when `merge_duplicates` collapses them.
///
/// Ordered cheapest → richest. `KeepExisting` is the historical behaviour
/// (drop the incoming copy); the others preserve information from both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MergeStrategy {
    /// Keep the entity already in the graph; discard the incoming duplicate.
    #[default]
    KeepExisting,
    /// Replace with the incoming duplicate (its id is rewritten to the canonical key).
    KeepIncoming,
    /// Union non-empty fields: max confidence, richer description, merged
    /// metadata, and a specific type over a generic `Other`.
    FieldUnion,
    /// Like `FieldUnion`, but when both descriptions are non-empty and differ,
    /// ask the LLM to synthesise one combined description. Falls back to
    /// `FieldUnion` when no backend is available or the call fails.
    Llm,
}

impl MergeStrategy {
    /// Whether resolving this strategy may require LLM calls.
    pub fn needs_backend(&self) -> bool {
        matches!(self, MergeStrategy::Llm)
    }
}

/// How aggressively duplicate entities are *recognised* before [`MergeStrategy`]
/// decides how to combine them.
///
/// Orthogonal to `MergeStrategy`: this picks *which* entities count as the same,
/// the strategy picks *how* their fields fold. `Off` is the historical behaviour
/// (exact lowercased-label match only). `Fuzzy` additionally collapses surface
/// variants of the same name — the main lever for cross-chunk coreference, where
/// the same entity surfaces as `"OpenAI"`, `"Open AI"` and `"OpenAI, Inc."` in
/// different segments and would otherwise fragment into three nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CorefMode {
    /// Only exact (case-insensitive) label matches are deduplicated.
    #[default]
    Off,
    /// Also merge labels that match after normalisation (case, punctuation,
    /// corporate suffixes, articles) or that are near-identical (high
    /// edit-distance similarity) and type-compatible.
    Fuzzy,
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
    /// How colliding duplicates are combined when `merge_duplicates` is set.
    #[serde(default)]
    pub merge_strategy: MergeStrategy,
    /// How duplicate entities are *recognised* before being combined. `Off`
    /// (default) matches exact lowercased labels only; `Fuzzy` also collapses
    /// surface variants of the same name for cross-chunk coreference.
    #[serde(default)]
    pub coref: CorefMode,
    /// An optional rich extraction *template* (preset). When set, schema-driven
    /// extractors render their prompt from the template's guideline and output
    /// fields (see [`crate::template`]) instead of the bare type-vocabulary,
    /// overriding [`mode`](Self::mode)'s prompt shaping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<crate::template::TemplateCfg>,
    /// Language to render a [`template`](Self::template) in. `None` uses the
    /// template's first declared language. Ignored without a template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for ExtractionSpec {
    fn default() -> Self {
        ExtractionSpec {
            schema: Schema::default(),
            mode: SchemaMode::Open,
            merge_duplicates: true,
            merge_strategy: MergeStrategy::default(),
            coref: CorefMode::default(),
            template: None,
            language: None,
        }
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
        ExtractionSpec {
            schema,
            mode,
            ..Default::default()
        }
    }

    /// A spec driven by a rich [`template`](crate::template::TemplateCfg)
    /// (preset), rendered in `lang` (`None` = the template's first declared
    /// language). The template guides extraction regardless of [`mode`](Self::mode).
    pub fn from_template(template: crate::template::TemplateCfg, lang: Option<String>) -> Self {
        ExtractionSpec {
            template: Some(template),
            language: lang,
            ..Default::default()
        }
    }
}
