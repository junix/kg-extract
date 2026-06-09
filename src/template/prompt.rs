//! Rendering a [`TemplateCfg`] into an extraction prompt.
//!
//! The bundled presets are far richer than the flat [`Schema`](crate::types::Schema)
//! type-vocabulary — they carry a target persona, per-field output descriptions
//! and natural-language rules. [`render_prompt`] folds all of that into a single
//! instruction block, while keeping the *output contract* identical to the
//! schema-driven extractors (`entities` / `relationships` / `entity_types` JSON)
//! so the existing [`GraphBuilder`](crate::graph_build::GraphBuilder) parses it
//! unchanged. The template thus steers *what* is extracted; the wire format stays
//! fixed.

use super::model::{Field, Guideline, TemplateCfg};

fn render_fields(heading: &str, intro: &str, fields: &[Field], lang: &str, out: &mut String) {
    if fields.is_empty() {
        return;
    }
    out.push_str(heading);
    out.push('\n');
    out.push_str(intro);
    out.push('\n');
    for f in fields {
        let opt = if f.required { "" } else { ", optional" };
        out.push_str(&format!(
            "- {} ({}{}): {}\n",
            f.name,
            f.field_type,
            opt,
            f.description.resolve(lang)
        ));
    }
    out.push('\n');
}

fn render_rules(heading: &str, rules: &Option<super::model::Localized>, lang: &str, out: &mut String) {
    if let Some(r) = rules {
        let body = r.resolve(lang);
        if !body.trim().is_empty() {
            out.push_str(heading);
            out.push('\n');
            out.push_str(&body);
            out.push_str("\n\n");
        }
    }
}

fn render_guideline_rules(g: &Guideline, lang: &str, out: &mut String) {
    // Naive (list/set/model) carries a single `rules`; graph-family splits them.
    render_rules("# Rules", &g.rules, lang, out);
    render_rules("# Rules for entities", &g.rules_for_entities, lang, out);
    render_rules("# Rules for relations", &g.rules_for_relations, lang, out);
    render_rules("# Rules for time", &g.rules_for_time, lang, out);
    render_rules("# Rules for location", &g.rules_for_location, lang, out);
}

/// Build the full extraction prompt for `template` in `lang` over `text`.
///
/// The output instruction is the same `entities` / `relationships` /
/// `entity_types` JSON contract the schema-driven extractors already parse, so a
/// template can drive any of them. Naive (`list`/`set`/`model`) templates have no
/// relations, so the model is told relationships may be empty.
pub fn render_prompt(template: &TemplateCfg, lang: &str, text: &str) -> String {
    let mut out = String::new();

    // Target persona / task framing.
    out.push_str(&template.guideline.target.resolve(lang));
    out.push_str("\n\n");

    // Output field schema.
    let entity_fields: &[Field] = template
        .output
        .entities
        .as_ref()
        .map(|g| g.fields.as_slice())
        .unwrap_or(&template.output.fields);
    render_fields(
        "# Entity fields",
        "Identify entities; for each capture these attributes:",
        entity_fields,
        lang,
        &mut out,
    );

    let has_relations = template
        .output
        .relations
        .as_ref()
        .map(|g| !g.fields.is_empty())
        .unwrap_or(false);
    if let Some(rel) = &template.output.relations {
        render_fields(
            "# Relation fields",
            "Identify relations between entities; each relation carries:",
            &rel.fields,
            lang,
            &mut out,
        );
    }

    // Natural-language rules.
    render_guideline_rules(&template.guideline, lang, &mut out);

    // Text + fixed output contract.
    out.push_str("# Text\n");
    out.push_str(text);
    out.push_str("\n\n");

    out.push_str(
        "# Output\n\
Return a single JSON object with exactly these keys:\n\
1. \"entities\": {\"<entity name>\": {\"type\": \"<entity type>\", \"attributes\": {\"<attr>\": \"<value>\"}}}\n\
2. \"relationships\": [[\"<subject>\", \"<predicate>\", \"<object>\"]]\n\
3. \"entity_types\": {\"<entity name>\": \"<entity type>\"}\n",
    );
    if !has_relations {
        out.push_str(
            "This template has no relations — return \"relationships\": [] (an empty list).\n",
        );
    }
    out.push_str("Use entity names exactly and consistently across all three keys. Output valid JSON only, no prose.\n");

    out
}
