use super::*;
use crate::types::{EntityType, Predicate, PredicateType};
#[allow(unused_imports)]
use schema_filter::{SchemaFilter, SchemaPolicy};

fn ent(id: &str, label: &str, ty: EntityType) -> Entity {
    Entity::new(id, label, ty)
}

fn tokens(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(n, t)| (n.to_lowercase(), t.to_string()))
        .collect()
}

#[test]
fn policy_dispatch_by_mode_and_schema() {
    let schema = Schema::new(vec!["PRODUCT".into()], vec![], vec![]);
    assert!(matches!(
        SchemaPolicy::for_mode(SchemaMode::Open, &schema),
        SchemaPolicy::Off
    ));
    assert!(matches!(
        SchemaPolicy::for_mode(SchemaMode::Fixed, &schema),
        SchemaPolicy::Fixed(_)
    ));
    assert!(matches!(
        SchemaPolicy::for_mode(SchemaMode::Evolving, &schema),
        SchemaPolicy::Evolving(_)
    ));
    // Empty schema degrades the constrained modes to Off.
    assert!(matches!(
        SchemaPolicy::for_mode(SchemaMode::Fixed, &Schema::default()),
        SchemaPolicy::Off
    ));
    assert!(matches!(
        SchemaPolicy::for_mode(SchemaMode::Evolving, &Schema::default()),
        SchemaPolicy::Off
    ));
}

#[test]
fn drops_out_of_schema_entity_and_its_dependent_relation() {
    // Only PRODUCT entities and USES relations are allowed.
    let schema = Schema::new(vec!["PRODUCT".into()], vec!["USES".into()], vec![]);
    let f = SchemaFilter::build(&schema).unwrap();

    let mut entities = HashMap::new();
    entities.insert("p".to_string(), ent("p", "Widget", EntityType::Product));
    entities.insert("o".to_string(), ent("o", "Acme", EntityType::Organization));

    // USES is allowed, but the object entity is dropped → relation must go too.
    let t = Triple::new(
        ent("p", "Widget", EntityType::Product),
        Predicate::with_label(PredicateType::Uses, "USES"),
        ent("o", "Acme", EntityType::Organization),
    );

    let toks = tokens(&[("widget", "PRODUCT"), ("acme", "ORGANIZATION")]);
    let sf = f.apply(entities, vec![t], &toks);

    assert_eq!(sf.kept_entities.len(), 1);
    assert!(sf.kept_entities.contains_key("p"));
    assert!(
        sf.kept_triples.is_empty(),
        "relation to a dropped endpoint must be dropped"
    );
    assert!(sf.dropped_types.contains("ORGANIZATION"));
    assert_eq!(sf.dropped_records, 2, "one entity + one relation");
    assert!(!sf.all_dropped(), "a product survived");
}

#[test]
fn drops_out_of_schema_relation_type_but_keeps_entities() {
    let schema = Schema::new(vec!["PRODUCT".into()], vec!["USES".into()], vec![]);
    let f = SchemaFilter::build(&schema).unwrap();
    let mut entities = HashMap::new();
    entities.insert("a".into(), ent("a", "Widget", EntityType::Product));
    entities.insert("b".into(), ent("b", "Gadget", EntityType::Product));
    // Both endpoints in-schema, but DEPENDS_ON is not an allowed relation.
    let t = Triple::new(
        ent("a", "Widget", EntityType::Product),
        Predicate::with_label(PredicateType::RelatedTo, "DEPENDS_ON"),
        ent("b", "Gadget", EntityType::Product),
    );
    let toks = tokens(&[("widget", "PRODUCT"), ("gadget", "PRODUCT")]);
    let sf = f.apply(entities, vec![t], &toks);
    assert_eq!(sf.kept_entities.len(), 2);
    assert!(sf.kept_triples.is_empty());
    assert!(sf.dropped_types.contains("DEPENDS_ON"));
}

#[test]
fn custom_schema_type_matches_via_raw_token() {
    // GADGET is not an EntityType variant — from_loose collapses it to Other,
    // so enum-level checking would wrongly drop it. The raw token rescues it.
    let schema = Schema::new(vec!["GADGET".into()], vec![], vec![]);
    let f = SchemaFilter::build(&schema).unwrap();
    let mut entities = HashMap::new();
    entities.insert("g".into(), ent("g", "Widget X", EntityType::Other));
    let toks = tokens(&[("widget x", "GADGET")]);
    let sf = f.apply(entities, vec![], &toks);
    assert_eq!(
        sf.kept_entities.len(),
        1,
        "custom type must match by raw token"
    );
    assert_eq!(sf.dropped_records, 0);
}

#[test]
fn all_dropped_flags_the_degenerate_slice() {
    let schema = Schema::new(vec!["PRODUCT".into()], vec![], vec![]);
    let f = SchemaFilter::build(&schema).unwrap();
    let mut entities = HashMap::new();
    entities.insert("o".into(), ent("o", "Acme", EntityType::Organization));
    let toks = tokens(&[("acme", "ORGANIZATION")]);
    let sf = f.apply(entities, vec![], &toks);
    assert!(sf.all_dropped(), "every record fell outside the schema");
}

#[test]
fn slice_prompt_uses_1_based_index_and_total() {
    let p = AgenticExtractor::slice_prompt(0, 3, "hello");
    assert!(
        p.contains("Slice 1/3:"),
        "first slice must be 1-based: {p:?}"
    );
    assert!(p.contains("hello"), "slice content is inlined");
    // Last slice.
    let p = AgenticExtractor::slice_prompt(2, 3, "tail");
    assert!(p.contains("Slice 3/3:"), "{p:?}");
}

#[test]
fn redo_prompt_carries_full_type_vocabulary() {
    let p = AgenticExtractor::redo_prompt("PRODUCT, ORG", "USES, FUNDS");
    assert!(p.contains("PRODUCT, ORG"), "{p:?}");
    assert!(p.contains("USES, FUNDS"), "{p:?}");
    // The redo template's sentinel phrase survives.
    assert!(p.contains("Redo THIS slice"), "{p:?}");
}

#[test]
fn drop_feedback_is_none_when_nothing_dropped() {
    // No dropped records -> no feedback (the caller's gate, but the helper
    // defends it too).
    assert!(AgenticExtractor::drop_feedback(0, &BTreeSet::new(), "X", "Y").is_none());
}

#[test]
fn drop_feedback_without_type_names_omits_the_dropped_slot() {
    // Records dropped but no recorded type names (e.g. only relation endpoints
    // were dropped) -> the `{dropped_types}` slot is empty, sentence reads
    // "...NOT in the schema. Stay...".
    let fb = AgenticExtractor::drop_feedback(2, &BTreeSet::new(), "PRODUCT", "USES").unwrap();
    assert!(fb.contains("discarded 2 record(s)"), "{fb:?}");
    // No stray "(dropped:" segment.
    assert!(!fb.contains("(dropped:"), "{fb:?}");
    assert!(fb.contains("PRODUCT"), "{fb:?}");
    assert!(fb.contains("USES"), "{fb:?}");
}

#[test]
fn drop_feedback_with_type_names_appends_dropped_csv() {
    // Types recorded -> slot becomes " (dropped: A, B)".
    let mut types = BTreeSet::new();
    types.insert("A".to_string());
    types.insert("B".to_string());
    let fb = AgenticExtractor::drop_feedback(3, &types, "PRODUCT", "USES").unwrap();
    assert!(fb.contains("discarded 3 record(s)"), "{fb:?}");
    assert!(fb.contains("(dropped: A, B)"), "{fb:?}");
    // BTreeSet keeps them sorted; both present.
    assert!(fb.contains("A"), "{fb:?}");
    assert!(fb.contains("B"), "{fb:?}");
}

#[test]
fn evolving_collects_types_outside_the_seed() {
    // Seed allows PRODUCT entities and USES relations. The model also emits an
    // ORGANIZATION entity and a DEPENDS_ON relation — Evolving keeps both but
    // reports them as proposed new types (nothing dropped).
    let schema = Schema::new(vec!["PRODUCT".into()], vec!["USES".into()], vec![]);
    let f = SchemaFilter::build(&schema).unwrap();
    let mut entities = HashMap::new();
    entities.insert("p".into(), ent("p", "Widget", EntityType::Product));
    entities.insert("o".into(), ent("o", "Acme", EntityType::Organization));
    let t = Triple::new(
        ent("p", "Widget", EntityType::Product),
        Predicate::with_label(PredicateType::RelatedTo, "DEPENDS_ON"),
        ent("o", "Acme", EntityType::Organization),
    );
    let toks = tokens(&[("widget", "PRODUCT"), ("acme", "ORGANIZATION")]);
    let (nodes, relations) = f.new_types(&entities, &[t], &toks);
    assert!(
        nodes.contains("ORGANIZATION"),
        "out-of-seed node must be proposed"
    );
    assert!(!nodes.contains("PRODUCT"), "seed node is not a proposal");
    assert!(
        relations.contains("DEPENDS_ON"),
        "out-of-seed relation must be proposed"
    );
    assert!(
        !relations.contains("USES"),
        "seed relation is not a proposal"
    );
}
