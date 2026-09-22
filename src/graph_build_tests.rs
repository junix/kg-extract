use super::*;

#[test]
fn passive_by_relation_swaps_actor_to_non_actor_direction() {
    let org = Entity::new("org", "Helio Systems", EntityType::Organization);
    let product = Entity::new("product", "Aurora Portal", EntityType::Product);
    let predicate = Predicate::new(PredicateType::DevelopedBy);

    assert!(should_swap_passive_by(&org, &predicate, &product));
    assert!(!should_swap_passive_by(&product, &predicate, &org));
}

#[test]
fn evidenced_by_swaps_evidence_source_to_claim_direction() {
    let mut logs = Entity::new("logs", "Audit Logs", EntityType::Other);
    logs.description = Some("Operational records used as evidence.".into());
    let mut reports = Entity::new("reports", "Incident Reports", EntityType::Other);
    reports.description = Some("Reports that are evidenced by logs.".into());
    let predicate = Predicate::new(PredicateType::EvidencedBy);

    assert!(should_swap_passive_by(&logs, &predicate, &reports));
    assert!(!should_swap_passive_by(&reports, &predicate, &logs));
}

#[test]
fn evidenced_by_does_not_swap_correct_direction_from_cross_descriptions() {
    let mut reports = Entity::new("reports", "Incident Reports", EntityType::Other);
    reports.description = Some("Reports evidenced by Audit Logs.".into());
    let mut logs = Entity::new("logs", "Audit Logs", EntityType::Other);
    logs.description = Some("Audit records evidencing Incident Reports.".into());
    let predicate = Predicate::new(PredicateType::EvidencedBy);

    assert!(!should_swap_passive_by(&reports, &predicate, &logs));
}

// --- entity_id: the deterministic shared id scheme ----------------------

#[test]
fn entity_id_is_entity_prefix_plus_first_eight_md5_hex() {
    // Pinned to the documented `entity_<md5(name)[..8]>` scheme so a graph
    // built by any extractor (or the MCP store) stays interchangeable.
    // Changing the digest or prefix silently breaks every stored id.
    assert_eq!(entity_id("OpenAI"), "entity_0523b132");
    assert_eq!(entity_id("GPT-4"), "entity_f7559929");
    assert_eq!(entity_id("Alice"), "entity_64489c85");
    // Format invariant: lowercase hex, exactly 8 chars after the prefix.
    let id = entity_id("arbitrary name");
    assert!(id.starts_with("entity_"));
    assert_eq!(id.len(), "entity_".len() + 8);
    assert!(id["entity_".len()..].chars().all(|c| c.is_ascii_hexdigit()));
    // Keyed on raw bytes (case-sensitive), as documented.
    assert_ne!(entity_id("Alice"), entity_id("alice"));
    // Deterministic across calls.
    assert_eq!(entity_id("OpenAI"), entity_id("OpenAI"));
}

// --- parse_entity_type / build_predicate: the tool/MCP type resolution --

#[test]
fn parse_entity_type_empty_or_unknown_falls_back_to_other() {
    assert_eq!(parse_entity_type(""), EntityType::Other);
    assert_eq!(
        parse_entity_type("   "),
        EntityType::Other,
        "input is trimmed first"
    );
    assert_eq!(
        parse_entity_type("totally unknown"),
        EntityType::Other,
        "unknown tokens fall back via from_loose"
    );
}

#[test]
fn parse_entity_type_exact_parse_then_loose_alias() {
    // Exact SCREAMING_SNAKE parse wins first.
    assert_eq!(parse_entity_type("PERSON"), EntityType::Person);
    assert_eq!(
        parse_entity_type("  ORGANIZATION  "),
        EntityType::Organization
    );
    // When the exact parse fails, from_loose aliasing resolves it.
    assert_eq!(parse_entity_type("Person"), EntityType::Person);
}

#[test]
fn build_predicate_normalises_separators_and_keeps_raw_label() {
    // Spaces/dashes -> underscores, upper-cased, then parsed.
    let p = build_predicate("developed by");
    assert_eq!(p.predicate_type, PredicateType::DevelopedBy);
    assert_eq!(p.label.as_deref(), Some("developed by"));
    assert_eq!(p.raw_type.as_deref(), Some("developed by"));
    assert_eq!(
        p.output_type(),
        "developed by",
        "raw label wins over the normalised value"
    );

    // Already-canonical upper form parses directly.
    assert_eq!(
        build_predicate("DEVELOPED_BY").predicate_type,
        PredicateType::DevelopedBy
    );
    // Dash separator normalises the same way.
    assert_eq!(
        build_predicate("is-used-by").predicate_type,
        PredicateType::IsUsedBy
    );
    // Unknown relation falls back to RelatedTo but still keeps the raw label.
    let unk = build_predicate("utterly fabricated relation");
    assert_eq!(unk.predicate_type, PredicateType::RelatedTo);
    assert_eq!(unk.label.as_deref(), Some("utterly fabricated relation"));
}

// --- GraphBuilder: dedup, dangling-drop, merge strategy, attributes -----

#[test]
fn graph_builder_dedups_entities_case_insensitively_with_stable_id() {
    let mut b = GraphBuilder::new();
    let id1 = b.add_entity_with_raw_type(
        "OpenAI",
        EntityType::Organization,
        None,
        None,
        HashMap::new(),
    );
    // A name differing only by case collides and returns the SAME id; the
    // shared md5 scheme means the id is the one computed from the first name.
    let id2 = b.add_entity_with_raw_type("openai", EntityType::Company, None, None, HashMap::new());
    assert_eq!(id1, id2);
    assert_eq!(id1, entity_id("OpenAI"));
    let g = b.into_graph();
    assert_eq!(
        g.entities.len(),
        1,
        "case-variant names collapse to one entity"
    );
    // KeepExisting (the default): first occurrence wins, the later one is discarded.
    let e = g.entities.values().next().unwrap();
    assert_eq!(e.entity_type, EntityType::Organization);
    assert_eq!(e.label, "OpenAI");
}

#[test]
fn graph_builder_add_relation_drops_dangling_and_resolves_case_insensitively() {
    let mut b = GraphBuilder::new();
    b.add_entity_with_raw_type("A", EntityType::Other, None, None, HashMap::new());
    // Dangling target -> nothing added, returns false.
    let added = b.add_relation("A", Predicate::new(PredicateType::Uses), "Ghost", |_| {});
    assert!(!added, "a relation to an unknown endpoint must be dropped");
    // Case-insensitive name resolution: "a" resolves to the entity added as "A".
    let added2 = b.add_relation("a", Predicate::new(PredicateType::Uses), "A", |_| {});
    assert!(added2);
    let g = b.into_graph();
    assert_eq!(
        g.triples.len(),
        1,
        "only the resolved relation survives; the dangling one is absent"
    );
}

#[test]
fn graph_builder_field_union_keeps_richer_description_and_specific_type() {
    let mut b = GraphBuilder::new().merge_strategy(MergeStrategy::FieldUnion);
    b.add_entity_with_raw_type(
        "Acme",
        EntityType::Organization,
        None,
        Some("short".into()),
        HashMap::new(),
    );
    b.add_entity_with_raw_type(
        "acme",
        EntityType::Other,
        None,
        Some("a much longer, richer description".into()),
        HashMap::new(),
    );
    let g = b.into_graph();
    assert_eq!(g.entities.len(), 1);
    let e = g.entities.values().next().unwrap();
    assert_eq!(
        e.entity_type,
        EntityType::Organization,
        "specific type beats Other"
    );
    assert_eq!(
        e.description.as_deref(),
        Some("a much longer, richer description"),
        "FieldUnion keeps the richer description"
    );
    // Canonical (first-occurrence) id survives the merge.
    assert_eq!(e.id, entity_id("Acme"));
}

#[test]
fn graph_builder_set_attribute_is_noop_on_unknown_case_insensitive_on_known() {
    // Unknown entity: no-op (no panic, no entity created).
    let mut b = GraphBuilder::new();
    b.set_attribute("Ghost", "k".into(), serde_json::json!(1));
    assert!(b.into_graph().entities.is_empty());

    // Known entity, looked up case-insensitively.
    let mut b = GraphBuilder::new();
    b.add_entity_with_raw_type("Node", EntityType::Other, None, None, HashMap::new());
    b.set_attribute("node", "score".into(), serde_json::json!(7));
    let g = b.into_graph();
    let e = g.entities.values().next().unwrap();
    assert_eq!(e.metadata["score"], serde_json::json!(7));
}
