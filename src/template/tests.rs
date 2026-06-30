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
    // Sanity: we shipped the full set (40 files; the original 37 plus the
    // Understand-Anything ports — code/codebase_graph,
    // code/business_domain_flow, knowledge/wiki_graph). The exact count catches
    // a silent drop on a parse failure as well as an unexpected duplicate.
    assert_eq!(presets.len(), 40, "expected the full preset set, got {}", presets.len());
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

#[test]
fn codebase_graph_preset_is_registered_and_renders() {
    // The code-understanding preset ports Understand-Anything's GraphNode/
    // GraphEdge schema into kg-extract's template form. It lives under the
    // `code/` domain (not `general/`), so only the qualified key resolves —
    // a bare name would be searched under `general/` per gallery::get's rule.
    let by_full = gallery::get("code/codebase_graph").expect("codebase_graph via full key");
    assert_eq!(by_full.autotype, AutoType::Graph);

    // The entity/relation `type` field docs carry the full UA vocabularies —
    // 13 node types and 29 edge types as enumerations in the field description.
    let node_type_doc = by_full
        .output
        .entities
        .as_ref()
        .expect("graph-family entities")
        .fields
        .iter()
        .find(|f| f.name == "type")
        .expect("entity type field");
    for t in [
        "file", "function", "class", "module", "concept", "config", "document", "service", "table",
        "endpoint", "pipeline", "schema", "resource",
    ] {
        assert!(
            node_type_doc.description.resolve("en").contains(t),
            "node type vocab missing '{t}'"
        );
    }
    let rel_type_doc = by_full
        .output
        .relations
        .as_ref()
        .expect("graph-family relations")
        .fields
        .iter()
        .find(|f| f.name == "type")
        .expect("relation type field");
    for t in [
        "imports",
        "exports",
        "contains",
        "inherits",
        "implements",
        "calls",
        "subscribes",
        "publishes",
        "middleware",
        "reads_from",
        "writes_to",
        "transforms",
        "validates",
        "depends_on",
        "tested_by",
        "configures",
        "related",
        "similar_to",
        "deploys",
        "serves",
        "provisions",
        "triggers",
        "migrates",
        "documents",
        "routes",
        "defines_schema",
    ] {
        assert!(
            rel_type_doc.description.resolve("en").contains(t),
            "relation type vocab missing '{t}'"
        );
    }

    // The rendered prompt keeps the schema-json wire format GraphBuilder parses.
    let lang = by_full.resolve_lang(Some("en"));
    assert_eq!(lang, "en");
    let prompt = render_prompt(&by_full, &lang, "src/main.ts imports src/utils.ts");
    assert!(prompt.contains("software architect"));
    assert!(prompt.contains("# Entity fields"));
    assert!(prompt.contains("# Relation fields"));
    assert!(prompt.contains("\"relationships\""));
    assert!(prompt.contains("src/main.ts imports src/utils.ts"));
}

#[test]
fn business_domain_flow_preset_is_registered_and_renders() {
    // Ports Understand-Anything's `/understand-domain` schema: domain/flow/step
    // nodes plus contains_flow/flow_step/cross_domain edges. Lives under `code/`.
    let tpl = gallery::get("code/business_domain_flow").expect("business_domain_flow via full key");
    assert_eq!(tpl.autotype, AutoType::TemporalGraph);

    let node_types = tpl
        .output
        .entities
        .as_ref()
        .expect("graph-family entities")
        .fields
        .iter()
        .find(|f| f.name == "type")
        .expect("entity type field")
        .description
        .resolve("en");
    for t in ["domain", "flow", "step"] {
        assert!(
            node_types.contains(t),
            "domain-flow node vocab missing '{t}'"
        );
    }
    let rel_types = tpl
        .output
        .relations
        .as_ref()
        .expect("graph-family relations")
        .fields
        .iter()
        .find(|f| f.name == "type")
        .expect("relation type field")
        .description
        .resolve("en");
    for t in ["contains_flow", "flow_step", "cross_domain"] {
        assert!(
            rel_types.contains(t),
            "domain-flow relation vocab missing '{t}'"
        );
    }
    // UA's domainMeta is carried as entity fields (entryType vocabulary too).
    for f in ["entryType", "businessRules", "crossDomainInteractions"] {
        assert!(
            tpl.output
                .entities
                .as_ref()
                .unwrap()
                .fields
                .iter()
                .any(|fld| fld.name == f),
            "domain-flow entity fields missing '{f}'"
        );
    }

    let prompt = render_prompt(&tpl, "en", "POST /checkout triggers the payment flow");
    assert!(prompt.contains("domain-driven-design"));
    assert!(prompt.contains("# Entity fields"));
    assert!(prompt.contains("\"relationships\""));
}

#[test]
fn wiki_graph_preset_is_registered_and_renders() {
    // Ports Understand-Anything's `/understand-knowledge` schema:
    // article/entity/topic/claim/source nodes plus the six knowledge edges.
    // Lives under a new `knowledge/` domain (bare key resolves under general/).
    let tpl = gallery::get("knowledge/wiki_graph").expect("wiki_graph via full key");
    assert_eq!(tpl.autotype, AutoType::Graph);

    let node_types = tpl
        .output
        .entities
        .as_ref()
        .expect("graph-family entities")
        .fields
        .iter()
        .find(|f| f.name == "type")
        .expect("entity type field")
        .description
        .resolve("en");
    for t in ["article", "entity", "topic", "claim", "source"] {
        assert!(node_types.contains(t), "wiki node vocab missing '{t}'");
    }
    let rel_types = tpl
        .output
        .relations
        .as_ref()
        .expect("graph-family relations")
        .fields
        .iter()
        .find(|f| f.name == "type")
        .expect("relation type field")
        .description
        .resolve("en");
    for t in [
        "cites",
        "contradicts",
        "builds_on",
        "exemplifies",
        "categorized_under",
        "authored_by",
    ] {
        assert!(rel_types.contains(t), "wiki relation vocab missing '{t}'");
    }
    // UA's knowledgeMeta is carried as entity fields.
    for f in ["wikilinks", "backlinks", "category", "content"] {
        assert!(
            tpl.output
                .entities
                .as_ref()
                .unwrap()
                .fields
                .iter()
                .any(|fld| fld.name == f),
            "wiki entity fields missing '{f}'"
        );
    }

    let prompt = render_prompt(&tpl, "en", "[[RAG]] cites a survey on retrieval.");
    assert!(prompt.contains("knowledge graph expert"));
    assert!(prompt.contains("# Entity fields"));
    assert!(prompt.contains("\"relationships\""));
}
