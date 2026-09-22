use crate::types::{Entity, EntityType, Predicate, PredicateType, Triple};

use super::to_ladybug_import_json;

#[test]
fn exports_generic_ladybug_import_document() {
    let mut openai = Entity::new("openai", "OpenAI", EntityType::Organization);
    openai.description = Some("AI lab".into());
    let gpt4 = Entity::new("gpt4", "GPT-4", EntityType::Technology);

    let mut kg = crate::types::KnowledgeGraph::new();
    kg.add_entity(openai.clone());
    kg.add_entity(gpt4.clone());
    kg.add_triple(Triple::new(
        openai,
        Predicate::new(PredicateType::DevelopedBy),
        gpt4,
    ));

    let doc = to_ladybug_import_json(&kg);
    assert_eq!(doc["format_version"], "graphdb-ladybug.export.v1");
    assert!(doc["schema"][0]
        .as_str()
        .unwrap()
        .contains("CREATE NODE TABLE KgEntity"));
    assert!(doc["schema"].as_array().unwrap().iter().any(|s| s
        .as_str()
        .unwrap()
        .contains("CREATE REL TABLE DEVELOPED_BY")));
    assert_eq!(doc["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(doc["relationships"][0]["_type"], "DEVELOPED_BY");
    assert_eq!(doc["relationships"][0]["_from_table"], "KgEntity");
    assert_eq!(doc["relationships"][0]["predicate"], "DEVELOPED_BY");
}
