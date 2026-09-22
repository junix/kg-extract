// Behavior-preservation guard: EntityType/TypeMatch now come from kg-vocab;
// these assertions must keep passing after the swap.
use super::*;

#[test]
fn roundtrip_values() {
    assert_eq!(EntityType::Person.value(), "PERSON");
    assert_eq!(EntityType::CelestialBody.value(), "CELESTIAL_BODY");
    assert_eq!(EntityType::BatchSize.value(), "BATCH_SIZE");
    assert_eq!("PERSON".parse::<EntityType>().unwrap(), EntityType::Person);
}

#[test]
fn loose_parse_fallbacks() {
    assert_eq!(EntityType::from_loose("model"), EntityType::Model);
    assert_eq!(EntityType::from_loose("method"), EntityType::Technology);
    assert_eq!(
        EntityType::from_loose("research group"),
        EntityType::Organization
    );
    assert_eq!(EntityType::from_loose("totally unknown"), EntityType::Other);
    assert_eq!(EntityType::from_loose("Person"), EntityType::Person);
}

#[test]
fn resolve_reports_match_kind() {
    assert_eq!(
        EntityType::resolve("person"),
        (EntityType::Person, TypeMatch::Exact)
    );
    assert_eq!(
        EntityType::resolve("LLM"),
        (EntityType::Technology, TypeMatch::Aliased)
    );
    assert_eq!(
        EntityType::resolve("research lab"),
        (EntityType::Institution, TypeMatch::Aliased)
    );
    assert_eq!(
        EntityType::resolve("researcher"),
        (EntityType::Person, TypeMatch::Aliased)
    );
    assert_eq!(
        EntityType::resolve("totally unknown"),
        (EntityType::Other, TypeMatch::Fallback)
    );
}

#[test]
fn variant_count() {
    assert_eq!(EntityType::all().len(), 122);
    assert_eq!(default_entity_types().len(), 122);
}
