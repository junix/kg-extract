//! The extraction-template data model — a faithful Rust port of the Hyper-Extract
//! `TemplateCfg` (see `hyperextract/utils/template_engine/parsers`).
//!
//! A *template* (a.k.a. preset) is a much richer description of an extraction
//! target than the flat [`Schema`](crate::types::Schema) type-vocabulary: it
//! carries the output field structure, a natural-language `guideline`
//! (target persona + extraction rules), identifier and display conventions, all
//! multilingual. The YAML on disk (`presets/**/*.yaml`) deserializes directly
//! into [`TemplateCfg`].
//!
//! Multilingual values are modelled by [`Localized`]; [`Localized::resolve`]
//! collapses one to a single-language string, mirroring the Python
//! `_localize_data` (lists become a numbered block).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The structural kind of a template, deciding how its `output`/`guideline` are
/// shaped (graph-family carry `entities`/`relations`; naive-family carry flat
/// `fields`). Mirrors the Python `VALID_AUTOTYPES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoType {
    Graph,
    Hypergraph,
    TemporalGraph,
    SpatialGraph,
    SpatioTemporalGraph,
    List,
    Set,
    Model,
}

impl AutoType {
    /// Graph-family types describe `entities` + `relations`; naive-family
    /// (`list`/`set`/`model`) describe a single flat list of `fields`.
    pub fn is_graph_family(self) -> bool {
        !matches!(self, AutoType::List | AutoType::Set | AutoType::Model)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AutoType::Graph => "graph",
            AutoType::Hypergraph => "hypergraph",
            AutoType::TemporalGraph => "temporal_graph",
            AutoType::SpatialGraph => "spatial_graph",
            AutoType::SpatioTemporalGraph => "spatio_temporal_graph",
            AutoType::List => "list",
            AutoType::Set => "set",
            AutoType::Model => "model",
        }
    }
}

/// A multilingual value: a bare string, an ordered list of strings, or a
/// per-language map (whose values are themselves a string or list).
///
/// [`resolve`](Localized::resolve) picks the requested language and renders it to
/// a single string — a list is rendered as a `1. … 2. …` numbered block, exactly
/// like the Python `_localize_data`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Localized {
    Text(String),
    List(Vec<String>),
    Map(BTreeMap<String, OneOrList>),
}

/// A string or a list of strings — the leaf of a per-language [`Localized::Map`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OneOrList {
    One(String),
    Many(Vec<String>),
}

fn number_list(items: &[String]) -> String {
    items
        .iter()
        .enumerate()
        .map(|(i, item)| format!("{}. {}", i + 1, item))
        .collect::<Vec<_>>()
        .join("\n")
}

impl OneOrList {
    fn render(&self) -> String {
        match self {
            OneOrList::One(s) => s.clone(),
            OneOrList::Many(items) => number_list(items),
        }
    }
}

impl Localized {
    /// Render to a single-language string. For a per-language map, prefer `lang`,
    /// then `en`, then any present language. Returns an empty string only when a
    /// map has no entries at all.
    pub fn resolve(&self, lang: &str) -> String {
        match self {
            Localized::Text(s) => s.clone(),
            Localized::List(items) => number_list(items),
            Localized::Map(m) => m
                .get(lang)
                .or_else(|| m.get("en"))
                .or_else(|| m.values().next())
                .map(OneOrList::render)
                .unwrap_or_default(),
        }
    }
}

/// Either a single language code or a list of supported ones (the YAML
/// `language:` key). Defaults to `[en]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Languages {
    One(String),
    Many(Vec<String>),
}

impl Default for Languages {
    fn default() -> Self {
        Languages::One("en".into())
    }
}

impl Languages {
    /// The declared languages, in order.
    pub fn list(&self) -> Vec<String> {
        match self {
            Languages::One(s) => vec![s.clone()],
            Languages::Many(v) => v.clone(),
        }
    }

    /// The first declared language (the template's default), or `en`.
    pub fn first(&self) -> String {
        match self {
            Languages::One(s) => s.clone(),
            Languages::Many(v) => v.first().cloned().unwrap_or_else(|| "en".into()),
        }
    }

    pub fn supports(&self, lang: &str) -> bool {
        self.list().iter().any(|l| l == lang)
    }
}

fn default_field_type() -> String {
    "str".into()
}

fn default_true() -> bool {
    true
}

/// One output field: a name, a (loose) type tag, a multilingual description, and
/// whether it is required.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type", default = "default_field_type")]
    pub field_type: String,
    pub description: Localized,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

/// A named, described group of fields (the naive `output`, or one of a graph
/// output's `entities` / `relations` sections).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldGroup {
    #[serde(default = "empty_localized")]
    pub description: Localized,
    #[serde(default)]
    pub fields: Vec<Field>,
}

fn empty_localized() -> Localized {
    Localized::Text(String::new())
}

/// The `output:` block. Naive templates populate `fields`; graph-family
/// templates populate `entities` and `relations`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Output {
    #[serde(default = "empty_localized")]
    pub description: Localized,
    // naive (list / set / model)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Field>,
    // graph family
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entities: Option<FieldGroup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relations: Option<FieldGroup>,
}

/// The `guideline:` block: a target persona plus rule lists. Naive templates use
/// `rules`; graph-family templates split rules by facet
/// (`rules_for_entities` / `_relations` / `_time` / `_location`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Guideline {
    pub target: Localized,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Localized>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_for_entities: Option<Localized>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_for_relations: Option<Localized>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_for_time: Option<Localized>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_for_location: Option<Localized>,
}

/// How members of a relation/hyperedge are named: a `{source, target}` pair
/// (binary graph), a list of participant field names (multi-role hyperedge, e.g.
/// `[sovereigns, ministers, …]`), or a single field holding a participant list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RelationMembers {
    Pair { source: String, target: String },
    Multi(Vec<String>),
    Single(String),
}

/// The optional `identifiers:` block — how stable ids are derived from fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Identifiers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation_members: Option<RelationMembers>,
}

/// The optional `options:` block. Kept loose (extra keys preserved) since
/// individual templates carry domain-specific switches; `extraction_mode` is the
/// common one (`two_stage`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Options {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction_mode: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// The optional `display:` block — label templates for rendering. Naive uses
/// `label`; graph-family uses `entity_label` / `relation_label`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Display {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation_label: Option<String>,
}

/// A complete extraction template, deserialized from a preset YAML file.
///
/// This is the multilingual form (descriptions/rules are [`Localized`]); call
/// [`resolve_lang`](TemplateCfg::resolve_lang) to pick a working language and
/// then render an extraction prompt via [`crate::template::render_prompt`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateCfg {
    #[serde(default)]
    pub language: Languages,
    pub name: String,
    #[serde(rename = "type")]
    pub autotype: AutoType,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "empty_localized")]
    pub description: Localized,
    pub output: Output,
    pub guideline: Guideline,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifiers: Option<Identifiers>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Options>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<Display>,
}

impl TemplateCfg {
    /// Parse a template from a YAML string.
    pub fn from_yaml_str(s: &str) -> anyhow::Result<TemplateCfg> {
        Ok(serde_yaml::from_str(s)?)
    }

    /// Parse a template from a YAML file on disk (the "bring your own template"
    /// path).
    pub fn from_yaml_file(path: impl AsRef<std::path::Path>) -> anyhow::Result<TemplateCfg> {
        let path = path.as_ref();
        let data = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading template {}: {e}", path.display()))?;
        Self::from_yaml_str(&data)
            .map_err(|e| anyhow::anyhow!("parsing template {}: {e}", path.display()))
    }

    /// Choose a concrete working language: the requested one if supported, else
    /// the template's first declared language.
    pub fn resolve_lang(&self, requested: Option<&str>) -> String {
        match requested {
            Some(l) if self.language.supports(l) => l.to_string(),
            _ => self.language.first(),
        }
    }

    /// One-line localized description (for `--list-presets`).
    pub fn describe(&self, lang: &str) -> String {
        self.description.resolve(lang)
    }
}
