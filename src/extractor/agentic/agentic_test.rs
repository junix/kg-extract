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
fn new_types_proposal_log_is_none_when_nothing_proposed() {
    assert!(new_types_proposal_log(0, &BTreeSet::new(), &BTreeSet::new()).is_none());
}

#[test]
fn new_types_proposal_log_lists_nodes_and_relations_with_1_based_slice() {
    let mut nn = BTreeSet::new();
    nn.insert("ORG".to_string());
    let mut nr = BTreeSet::new();
    nr.insert("DEPENDS_ON".to_string());
    // slice_index 2 -> "slice 3" (1-based).
    let line = new_types_proposal_log(2, &nn, &nr).unwrap();
    assert!(line.contains("slice 3"), "{line:?}");
    assert!(line.contains("2 new type(s)"), "{line:?}");
    // Both proposals appear, nodes joined before relations.
    assert!(line.contains("ORG, DEPENDS_ON"), "{line:?}");
}

#[test]
fn stamp_slice_citations_marks_entity_triple_and_both_endpoints() {
    use crate::citation::{Citation, CITATIONS_KEY};
    let cite = Citation::new(Some("doc.md".into()), 5, 9);

    let mut parsed = ParsedResult::default();
    parsed.entities.insert(
        "p".to_string(),
        ent("p", "Widget", EntityType::Product),
    );
    let t = Triple::new(
        ent("s", "Src", EntityType::Product),
        Predicate::with_label(PredicateType::Uses, "USES"),
        ent("o", "Obj", EntityType::Organization),
    );
    parsed.triples.push(t);

    stamp_slice_citations(&mut parsed, &cite);

    // Entity stamped.
    let e_cites = parsed.entities["p"]
        .metadata
        .get(CITATIONS_KEY)
        .and_then(|v| v.as_array())
        .expect("entity cited");
    assert_eq!(e_cites.len(), 1);

    // Triple + both endpoints stamped (the add_triple re-insert path needs all
    // three, else an unstamped snapshot erases provenance on union).
    let t = &parsed.triples[0];
    for meta in [&t.metadata, &t.subject.metadata, &t.object.metadata] {
        assert!(
            meta.get(CITATIONS_KEY).is_some(),
            "triple endpoint metadata must be cited"
        );
    }
}

#[test]
fn stamp_slice_citations_does_not_duplicate_an_identical_citation() {
    use crate::citation::{Citation, CITATIONS_KEY};
    let cite = Citation::new(Some("doc.md".into()), 1, 4);
    let mut parsed = ParsedResult::default();
    parsed.entities.insert(
        "p".to_string(),
        ent("p", "Widget", EntityType::Product),
    );

    stamp_slice_citations(&mut parsed, &cite);
    stamp_slice_citations(&mut parsed, &cite); // same citation again

    let count = parsed.entities["p"]
        .metadata
        .get(CITATIONS_KEY)
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(count, 1, "attach_citation skips exact duplicates");
}

#[test]
fn commit_unconstrained_slice_merges_entities_and_appends_triples() {
    let mut all_entities: HashMap<String, Entity> = HashMap::new();
    let mut all_triples: Vec<Triple> = Vec::new();
    let mut parsed_results: Vec<ParsedResult> = Vec::new();

    let mut parsed = ParsedResult::default();
    parsed.entities.insert(
        "p".to_string(),
        ent("p", "Widget", EntityType::Product),
    );
    let t = Triple::new(
        ent("p", "Widget", EntityType::Product),
        Predicate::with_label(PredicateType::Uses, "USES"),
        ent("o", "Acme", EntityType::Organization),
    );
    parsed.triples.push(t);

    commit_unconstrained_slice(
        &mut all_entities,
        &mut all_triples,
        &mut parsed_results,
        parsed,
    );
    assert_eq!(all_entities.len(), 1, "entity merged into the table");
    assert_eq!(all_triples.len(), 1, "triple appended");
    assert_eq!(parsed_results.len(), 1, "parse retained");
}

#[test]
fn commit_unconstrained_slice_first_occurrence_wins_on_repeat() {
    // A repeat entity must keep its first id (first-occurrence-wins) but still
    // union citations; a brand-new id is inserted.
    let mut all_entities: HashMap<String, Entity> = HashMap::new();
    all_entities.insert(
        "p".to_string(),
        ent("p", "Widget", EntityType::Product),
    );
    let mut all_triples: Vec<Triple> = Vec::new();
    let mut parsed_results: Vec<ParsedResult> = Vec::new();

    let mut parsed = ParsedResult::default();
    parsed.entities.insert(
        "p".to_string(),
        ent("p", "Widget repeated", EntityType::Product),
    );
    parsed.entities.insert(
        "q".to_string(),
        ent("q", "Gadget", EntityType::Product),
    );

    commit_unconstrained_slice(
        &mut all_entities,
        &mut all_triples,
        &mut parsed_results,
        parsed,
    );
    assert_eq!(all_entities.len(), 2, "repeat + new = 2 entities");
    // First occurrence's label is kept (provenance unions, identity does not).
    assert_eq!(all_entities["p"].label, "Widget");
    assert_eq!(all_entities["q"].label, "Gadget");
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

// ---- assemble_response: the three schema-policy arms are pure over their
// inputs, so they can be exercised without an SDK client. ----

fn outcome() -> SessionOutcome {
    let mut entities = HashMap::new();
    entities.insert(
        "p".to_string(),
        ent("p", "Widget", EntityType::Product),
    );
    SessionOutcome {
        slices_count: 2,
        total_tool_uses: 3,
        entities,
        triples: Vec::new(),
        parsed_results: Vec::new(),
        total_dropped: 0,
        dropped_types: BTreeSet::new(),
        new_nodes: BTreeSet::new(),
        new_relations: BTreeSet::new(),
    }
}

fn config_for(mode: SchemaMode) -> ExtractionConfig {
    let mut c = AgenticExtractor::default_config();
    c.spec.mode = mode;
    c
}

#[test]
fn assemble_response_off_policy_has_no_schema_metadata() {
    let cfg = config_for(SchemaMode::Open);
    let resp = AgenticExtractor::assemble_response(outcome(), &SchemaPolicy::Off, true, SchemaMode::Open, &cfg);
    // Common metadata always present.
    assert_eq!(resp.metadata["tool_uses"], serde_json::json!(3));
    assert_eq!(resp.metadata["schema_mode"], serde_json::json!("open"));
    // Off arm sets neither fixed- nor evolving-specific keys.
    assert!(resp.metadata.get("schema_dropped_records").is_none());
    assert!(resp.metadata.get("new_schema_types").is_none());
    // The single Product entity made it into the knowledge graph.
    assert_eq!(resp.knowledge_graph.entities.len(), 1);
}

#[test]
fn assemble_response_fixed_policy_records_drops() {
    let cfg = config_for(SchemaMode::Fixed);
    let mut o = outcome();
    o.total_dropped = 4;
    let mut dt = BTreeSet::new();
    dt.insert("ORGANIZATION".to_string());
    dt.insert("PERSON".to_string());
    o.dropped_types = dt;
    let schema = Schema::new(vec!["PRODUCT".into()], vec!["USES".into()], vec![]);
    let filter = SchemaFilter::build(&schema).expect("non-empty schema builds a filter");
    let resp = AgenticExtractor::assemble_response(o, &SchemaPolicy::Fixed(filter), true, SchemaMode::Fixed, &cfg);
    assert_eq!(resp.metadata["schema_dropped_records"], serde_json::json!(4));
    // BTreeSet -> sorted Vec; both dropped types recorded.
    assert_eq!(
        resp.metadata["schema_dropped_types"],
        serde_json::json!(["ORGANIZATION", "PERSON"])
    );
    // Fixed arm must not emit the evolving `new_schema_types` shape.
    assert!(resp.metadata.get("new_schema_types").is_none());
}

#[test]
fn assemble_response_evolving_policy_proposes_new_types() {
    let cfg = config_for(SchemaMode::Evolving);
    let mut o = outcome();
    let mut nn = BTreeSet::new();
    nn.insert("GADGET".to_string());
    o.new_nodes = nn;
    let mut nr = BTreeSet::new();
    nr.insert("DEPENDS_ON".to_string());
    o.new_relations = nr;
    let schema = Schema::new(vec!["PRODUCT".into()], vec!["USES".into()], vec![]);
    let filter = SchemaFilter::build(&schema).expect("non-empty schema builds a filter");
    let resp = AgenticExtractor::assemble_response(o, &SchemaPolicy::Evolving(filter), true, SchemaMode::Evolving, &cfg);
    let nst = resp.metadata.get("new_schema_types").expect("evolving sets new_schema_types");
    // Mirrors SchemaJson/ToolCall's shape: nodes/relations sorted, attributes empty.
    assert_eq!(nst["nodes"], serde_json::json!(["GADGET"]));
    assert_eq!(nst["relations"], serde_json::json!(["DEPENDS_ON"]));
    assert_eq!(nst["attributes"], serde_json::json!([]));
    // Evolving arm must not emit the fixed drop-count keys.
    assert!(resp.metadata.get("schema_dropped_records").is_none());
    assert!(resp.metadata.get("schema_dropped_types").is_none());
}

#[test]
fn assemble_response_carries_config_and_parsed_results() {
    let cfg = config_for(SchemaMode::Open);
    let mut o = outcome();
    // parsed_results survives into the response verbatim (empty here, but the
    // field wiring is what we're checking).
    let _ = std::mem::replace(&mut o.parsed_results, Vec::new());
    let resp = AgenticExtractor::assemble_response(o, &SchemaPolicy::Off, true, SchemaMode::Open, &cfg);
    assert!(resp.config.is_some(), "config is stamped on the response");
    // source_doc defaults to None in default_config; this just confirms the
    // cloned config round-trips.
    assert!(resp.parsed_results.is_empty());
}
