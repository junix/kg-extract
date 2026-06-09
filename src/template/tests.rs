//! Tests for the template engine: model/localization, the embedded gallery, and
//! prompt rendering.

use super::*;

#[test]
fn localized_resolves_text_list_and_map() {
    let text = Localized::Text("hello".into());
    assert_eq!(text.resolve("zh"), "hello");

    let list = Localized::List(vec!["a".into(), "b".into()]);
    assert_eq!(list.resolve("en"), "1. a\n2. b");

    let mut m = std::collections::BTreeMap::new();
    m.insert("zh".to_string(), OneOrList::One("你好".into()));
    m.insert("en".to_string(), OneOrList::One("hi".into()));
    let map = Localized::Map(m);
    assert_eq!(map.resolve("zh"), "你好");
    assert_eq!(map.resolve("en"), "hi");
    // Unknown language falls back to en, then any.
    assert_eq!(map.resolve("fr"), "hi");
}

#[test]
fn every_bundled_preset_parses() {
    let presets = gallery::list();
    // Sanity: we shipped the full set (37 files at port time).
    assert!(presets.len() >= 37, "only {} presets loaded", presets.len());
    for p in presets {
        assert!(p.key.contains('/'), "key not domain-qualified: {}", p.key);
        // Each declares at least a target persona.
        assert!(!p.template.guideline.target.resolve("en").is_empty());
    }
}

#[test]
fn gallery_get_resolves_bare_and_qualified_keys() {
    // base_graph.yaml has `name: graph` under general/ → key general/graph.
    let by_bare = gallery::get("graph").expect("general/graph via bare name");
    let by_full = gallery::get("general/graph").expect("general/graph via full key");
    assert_eq!(by_bare, by_full);
    assert_eq!(by_full.autotype, AutoType::Graph);

    assert!(gallery::get("does/not-exist").is_none());
}

#[test]
fn concept_graph_renders_a_template_prompt() {
    let tpl = gallery::get("general/concept_graph").expect("concept_graph preset");
    let lang = tpl.resolve_lang(Some("en"));
    assert_eq!(lang, "en");
    let prompt = render_prompt(&tpl, &lang, "Photosynthesis is a process.");

    // Persona + field schema + rules + the fixed output contract are all present.
    assert!(prompt.contains("knowledge graph expert"));
    assert!(prompt.contains("# Entity fields"));
    assert!(prompt.contains("# Relation fields"));
    assert!(prompt.contains("\"relationships\""));
    assert!(prompt.contains("Photosynthesis is a process."));
}

#[test]
fn naive_template_marks_relationships_empty() {
    // `list` is a naive type with no relations.
    let tpl = gallery::get("general/list").expect("list preset");
    assert_eq!(tpl.autotype, AutoType::List);
    let prompt = render_prompt(&tpl, "en", "some text");
    assert!(prompt.contains("# Entity fields"));
    assert!(!prompt.contains("# Relation fields"));
    assert!(prompt.contains("empty list"));
}

#[test]
fn resolve_lang_falls_back_to_first_declared() {
    let tpl = gallery::get("general/graph").unwrap();
    // zh/en declared; an undeclared language falls back to the first (zh).
    assert_eq!(tpl.resolve_lang(Some("fr")), "zh");
    assert_eq!(tpl.resolve_lang(None), "zh");
    assert_eq!(tpl.resolve_lang(Some("en")), "en");
}

#[test]
fn template_roundtrips_through_json() {
    // Templates ride inside ExtractionSpec, which is JSON-serialized; ensure the
    // model survives a round-trip.
    let tpl = gallery::get("finance/event_timeline").expect("event_timeline preset");
    let json = serde_json::to_string(&tpl).unwrap();
    let back: TemplateCfg = serde_json::from_str(&json).unwrap();
    assert_eq!(tpl, back);
}
