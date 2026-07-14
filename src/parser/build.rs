use std::collections::HashMap;

use crate::types::{Entity, EntityType, Predicate, PredicateType, Triple};

use super::EntityInfo;

/// Build [`Entity`] objects from parsed entity-info.
pub fn create_entities_from_parsed(
    entities: &HashMap<String, EntityInfo>,
) -> HashMap<String, Entity> {
    let mut result = HashMap::new();
    for (id, data) in entities {
        let label = if data.label.is_empty() {
            id.clone()
        } else {
            data.label.clone()
        };

        let entity_type = if let Some(type_str) = &data.r#type {
            EntityType::from_loose(type_str)
        } else {
            heuristic_type(&label)
        };

        let mut entity = Entity::new(id.clone(), label, entity_type);
        entity.raw_type = data.r#type.clone();
        entity.description = data.description.clone();
        if !data.attributes.is_empty() {
            entity.metadata = data.attributes.clone();
        }
        result.insert(id.clone(), entity);
    }
    result
}

fn heuristic_type(label: &str) -> EntityType {
    let label = label.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| label.contains(word));
    if has(&["person", "people", "human"]) {
        EntityType::Person
    } else if has(&["company", "corporation", "inc", "ltd"]) {
        EntityType::Company
    } else if has(&["organization", "institute", "agency"]) {
        EntityType::Organization
    } else if has(&["city", "town", "village"]) {
        EntityType::City
    } else if has(&["country", "nation", "state"]) {
        EntityType::Country
    } else if has(&["technology", "software", "system"]) {
        EntityType::Technology
    } else {
        EntityType::PhysicalObject
    }
}

/// Build [`Triple`] objects from relationship tuples. Skips relations whose
/// endpoints are unknown.
pub fn create_triples_from_parsed(
    relationships: &[(String, String, String)],
    entities: &HashMap<String, Entity>,
) -> Vec<Triple> {
    let mut triples = Vec::new();
    for (source_id, relation, target_id) in relationships {
        let (Some(subject), Some(object)) = (entities.get(source_id), entities.get(target_id))
        else {
            continue;
        };
        let predicate_type = PredicateType::from_loose(relation);
        let predicate = Predicate::with_label(predicate_type, relation.clone());
        triples.push(Triple::new(subject.clone(), predicate, object.clone()));
    }
    triples
}
