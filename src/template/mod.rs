//! Extraction **templates** (a.k.a. presets) — a richer, multilingual
//! description of an extraction target than the flat [`Schema`](crate::types::Schema).
//!
//! A template bundles the output field structure, a natural-language `guideline`
//! (target persona + extraction rules), and identifier/display conventions. The
//! crate ships a gallery of presets (`presets/**/*.yaml`, embedded into the
//! binary), and users may also supply their own template file.
//!
//! - [`model`] — the data model ([`TemplateCfg`] and friends) + YAML loading.
//! - [`gallery`] — the bundled presets, looked up by `{domain}/{name}` key.
//! - [`prompt`] — render a template into an extraction prompt that yields the
//!   same JSON contract the schema-driven extractors already parse.
//!
//! ```no_run
//! use kg_extract::template::{gallery, render_prompt, TemplateCfg};
//!
//! // Load a bundled preset by name…
//! let tpl = gallery::get("general/concept_graph").unwrap();
//! // …or a user-supplied one: TemplateCfg::from_yaml_file("my.yaml").
//! let lang = tpl.resolve_lang(Some("en"));
//! let prompt = render_prompt(&tpl, &lang, "some input text");
//! ```

pub mod gallery;
pub mod model;
pub mod prompt;

pub use gallery::Preset;
pub use model::{
    AutoType, Display, Field, FieldGroup, Guideline, Identifiers, Languages, Localized, OneOrList,
    Options, Output, RelationMembers, TemplateCfg,
};
pub use prompt::render_prompt;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
