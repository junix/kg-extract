use super::*;

/// A fresh, unique temp output dir for one test (cleaned up on drop).
struct TmpStore {
    dir: PathBuf,
    source_dir: PathBuf,
    store: KgStore,
}

impl TmpStore {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("kg-mcp-test-{}", nanoid::nanoid!()));
        let source_dir = dir.join("source");
        std::fs::create_dir_all(&source_dir).unwrap();
        let store = KgStore::with_policy_and_source_root(
            dir.clone(),
            SchemaPolicy::open(),
            source_dir.clone(),
        );
        TmpStore {
            dir,
            source_dir,
            store,
        }
    }

    fn with_policy(policy: SchemaPolicy) -> Self {
        let dir = std::env::temp_dir().join(format!("kg-mcp-test-{}", nanoid::nanoid!()));
        let source_dir = dir.join("source");
        std::fs::create_dir_all(&source_dir).unwrap();
        let store = KgStore::with_policy_and_source_root(dir.clone(), policy, source_dir.clone());
        TmpStore {
            dir,
            source_dir,
            store,
        }
    }

    fn write_source(&self, rel: &str, lines: usize) {
        let path = self.source_dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let body = (1..=lines)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(path, body).unwrap();
    }
}

impl Drop for TmpStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn n(v: &Value, key: &str) -> u64 {
    v["stats"][key].as_u64().unwrap()
}

#[test]
fn resolve_appends_json_and_rejects_escape() {
    let t = TmpStore::new();
    let s = &t.store;
    assert_eq!(s.resolve("foo").unwrap(), t.dir.join("foo.json"));
    // Caller-supplied .json is not doubled.
    assert_eq!(s.resolve("foo.json").unwrap(), t.dir.join("foo.json"));
    // Nested paths create subdirs.
    assert_eq!(
        s.resolve("a/b/c").unwrap(),
        t.dir.join("a").join("b").join("c.json")
    );
    // Traversal and empties are rejected.
    assert!(s.resolve("../escape").is_err());
    assert!(s.resolve("a/../../b").is_err());
    assert!(s.resolve("").is_err());
    assert!(s.resolve("/").is_err());
    // Absolute paths are rejected (per the doc), not silently made relative.
    assert!(
        s.resolve("/etc/graph").is_err(),
        "absolute unix path must be rejected"
    );
    assert!(
        s.resolve("\\abs\\path").is_err(),
        "absolute windows-style path must be rejected"
    );
}

#[test]
fn add_entity_persists_and_reloads() {
    let t = TmpStore::new();
    let r = t
        .store
        .add_entity(
            "g",
            "Alice",
            "person",
            Some("An engineer".into()),
            HashMap::new(),
        )
        .unwrap();
    assert_eq!(n(&r, "num_entities"), 1);

    // File exists and round-trips through serde.
    let path = t.store.resolve("g").unwrap();
    assert!(path.exists());
    let kg = t.store.load("g").unwrap();
    let e = kg.get_entity(&entity_id("Alice")).unwrap();
    assert_eq!(e.label, "Alice");
    assert_eq!(e.description.as_deref(), Some("An engineer"));
    assert_eq!(e.entity_type.value().to_lowercase(), "person");
}

#[test]
fn add_entity_same_name_merges_not_duplicates() {
    let t = TmpStore::new();
    t.store
        .add_entity("g", "Alice", "person", None, HashMap::new())
        .unwrap();
    let mut attrs = HashMap::new();
    attrs.insert("role".to_string(), serde_json::json!("VP"));
    let r = t
        .store
        .add_entity("g", "Alice", "person", Some("Updated".into()), attrs)
        .unwrap();
    assert_eq!(
        n(&r, "num_entities"),
        1,
        "same name must merge, not duplicate"
    );

    let kg = t.store.load("g").unwrap();
    let e = kg.get_entity(&entity_id("Alice")).unwrap();
    assert_eq!(e.description.as_deref(), Some("Updated"));
    assert_eq!(e.metadata.get("role").unwrap(), &serde_json::json!("VP"));
}

#[test]
fn add_entity_with_citation_merges_provenance() {
    let t = TmpStore::new();
    t.write_source("doc.md", 12);
    t.store
        .add_entity_with_citation(
            "g",
            "Alice",
            "person",
            None,
            HashMap::new(),
            Some(SourceCitation::new("doc.md".into(), 3, 5).unwrap()),
        )
        .unwrap();
    t.store
        .add_entity_with_citation(
            "g",
            "Alice",
            "person",
            None,
            HashMap::new(),
            Some(SourceCitation::new("doc.md".into(), 9, 12).unwrap()),
        )
        .unwrap();

    let kg = t.store.load("g").unwrap();
    let e = kg.get_entity(&entity_id("Alice")).unwrap();
    assert_eq!(
        e.metadata.get(crate::citation::CITATIONS_KEY).unwrap(),
        &serde_json::json!([
            {"doc": "doc.md", "lines": [3, 5]},
            {"doc": "doc.md", "lines": [9, 12]},
        ])
    );
}

#[test]
fn add_entity_with_citation_rejects_bad_source_file() {
    let t = TmpStore::new();
    let err = t
        .store
        .add_entity_with_citation(
            "g",
            "Alice",
            "person",
            None,
            HashMap::new(),
            Some(SourceCitation::new("missing.md".into(), 1, 1).unwrap()),
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("source_file 'missing.md' was not found"),
        "unexpected error: {err}"
    );
    assert!(!t.store.resolve("g").unwrap().exists());
}

#[test]
fn add_entity_with_citation_rejects_out_of_range_lines() {
    let t = TmpStore::new();
    t.write_source("doc.md", 2);
    let err = t
        .store
        .add_entity_with_citation(
            "g",
            "Alice",
            "person",
            None,
            HashMap::new(),
            Some(SourceCitation::new("doc.md".into(), 1, 3).unwrap()),
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("exceed source_file 'doc.md' line count 2"),
        "unexpected error: {err}"
    );
    assert!(!t.store.resolve("g").unwrap().exists());
}

#[test]
fn source_citation_rejects_absolute_and_parent_paths() {
    assert!(SourceCitation::new("/tmp/doc.md".into(), 1, 1).is_err());
    assert!(SourceCitation::new("../doc.md".into(), 1, 1).is_err());
    assert!(SourceCitation::new("a/../doc.md".into(), 1, 1).is_err());
}

#[test]
fn add_relation_errors_on_missing_endpoint_with_guidance() {
    let t = TmpStore::new();
    t.store
        .add_entity("g", "Alice", "person", None, HashMap::new())
        .unwrap();
    // Target 'Acme' doesn't exist → strict error, naming it + listing knowns.
    let err = t
        .store
        .add_relation("g", "Alice", "works_at", "Acme", None, None)
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("'Acme'"), "names the missing endpoint: {msg}");
    assert!(msg.contains("add_entity"), "prescribes the fix: {msg}");
    assert!(msg.contains("Alice"), "lists known entities: {msg}");
    // Nothing partially written: graph still has just Alice, no triple.
    let kg = t.store.load("g").unwrap();
    assert_eq!(kg.entities.len(), 1);
    assert_eq!(kg.triples.len(), 0);
}

#[test]
fn add_relation_succeeds_after_endpoints_added_and_dedups() {
    let t = TmpStore::new();
    t.store
        .add_entity("g", "Alice", "person", None, HashMap::new())
        .unwrap();
    t.store
        .add_entity("g", "Acme", "organization", None, HashMap::new())
        .unwrap();
    let r = t
        .store
        .add_relation("g", "Alice", "works_at", "Acme", None, Some(0.9))
        .unwrap();
    assert_eq!(n(&r, "num_entities"), 2);
    assert_eq!(n(&r, "num_triples"), 1);

    // Identical relation again must not duplicate the triple.
    let r2 = t
        .store
        .add_relation("g", "Alice", "works_at", "Acme", None, None)
        .unwrap();
    assert_eq!(n(&r2, "num_triples"), 1, "identical relation must dedup");
    assert!(r2["message"].as_str().unwrap().contains("already present"));
}

#[test]
fn add_relation_with_citation_dedups_and_merges_provenance() {
    let t = TmpStore::new();
    t.write_source("doc.md", 31);
    t.store
        .add_entity("g", "Alice", "person", None, HashMap::new())
        .unwrap();
    t.store
        .add_entity("g", "Acme", "organization", None, HashMap::new())
        .unwrap();
    t.store
        .add_relation_with_citation(
            "g",
            "Alice",
            "works_at",
            "Acme",
            None,
            Some(0.9),
            Some(SourceCitation::new("doc.md".into(), 20, 22).unwrap()),
        )
        .unwrap();
    t.store
        .add_relation_with_citation(
            "g",
            "Alice",
            "works_at",
            "Acme",
            None,
            Some(0.9),
            Some(SourceCitation::new("doc.md".into(), 30, 31).unwrap()),
        )
        .unwrap();

    let kg = t.store.load("g").unwrap();
    assert_eq!(kg.triples.len(), 1);
    assert_eq!(
        kg.triples[0]
            .metadata
            .get(crate::citation::CITATIONS_KEY)
            .unwrap(),
        &serde_json::json!([
            {"doc": "doc.md", "lines": [20, 22]},
            {"doc": "doc.md", "lines": [30, 31]},
        ])
    );
}

#[test]
fn add_relation_after_add_entity_keeps_rich_entity() {
    let t = TmpStore::new();
    t.store
        .add_entity(
            "g",
            "Alice",
            "person",
            Some("An engineer".into()),
            HashMap::new(),
        )
        .unwrap();
    t.store
        .add_entity("g", "Acme", "organization", None, HashMap::new())
        .unwrap();
    t.store
        .add_relation("g", "Alice", "works_at", "Acme", None, None)
        .unwrap();

    let kg = t.store.load("g").unwrap();
    // add_triple re-inserts endpoints; the enriched Alice must survive.
    let alice = kg.get_entity(&entity_id("Alice")).unwrap();
    assert_eq!(alice.description.as_deref(), Some("An engineer"));
    assert_eq!(alice.entity_type.value().to_lowercase(), "person");
}

#[test]
fn add_relation_clamps_strength_to_unit_interval() {
    let t = TmpStore::new();
    t.store
        .add_entity("g", "Alice", "person", None, HashMap::new())
        .unwrap();
    t.store
        .add_entity("g", "Acme", "organization", None, HashMap::new())
        .unwrap();
    t.store
        .add_relation("g", "Alice", "works_at", "Acme", None, Some(2.0))
        .unwrap();
    let kg = t.store.load("g").unwrap();
    assert_eq!(
        kg.triples[0].confidence,
        Some(1.0),
        "out-of-range strength must clamp to 1.0"
    );
}

#[test]
fn add_attribute_requires_existing_entity() {
    let t = TmpStore::new();
    t.store
        .add_entity("g", "Alice", "person", None, HashMap::new())
        .unwrap();
    let err = t
        .store
        .add_attribute("g", "Ghost", "k", serde_json::json!("v"))
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("'Ghost'") && msg.contains("add_entity"),
        "actionable error: {msg}"
    );
    assert!(msg.contains("Alice"), "lists known entities: {msg}");

    t.store
        .add_attribute("g", "Alice", "team", serde_json::json!("platform"))
        .unwrap();
    let kg = t.store.load("g").unwrap();
    let e = kg.get_entity(&entity_id("Alice")).unwrap();
    assert_eq!(
        e.metadata.get("team").unwrap(),
        &serde_json::json!("platform")
    );
}

/// Build a small graph: Alice —works_at→ Acme, Alice —knows→ Bob.
fn seed_graph(s: &KgStore) {
    s.add_entity(
        "g",
        "Alice",
        "person",
        Some("An engineer".into()),
        HashMap::new(),
    )
    .unwrap();
    s.add_entity("g", "Acme", "organization", None, HashMap::new())
        .unwrap();
    s.add_entity("g", "Bob", "person", None, HashMap::new())
        .unwrap();
    s.add_relation("g", "Alice", "works_at", "Acme", None, None)
        .unwrap();
    s.add_relation("g", "Alice", "knows", "Bob", None, None)
        .unwrap();
}

#[test]
fn query_summary_is_default_and_counts_only() {
    let t = TmpStore::new();
    seed_graph(&t.store);
    let r = t.store.query_graph("g", "summary", None, 200).unwrap();
    assert_eq!(r["stats"]["num_entities"].as_u64().unwrap(), 3);
    assert_eq!(r["stats"]["num_triples"].as_u64().unwrap(), 2);
    // summary carries no entity/relation arrays (cheap by design).
    assert!(r.get("entities").is_none() && r.get("relations").is_none());
}

#[test]
fn query_entities_only_and_relations_only() {
    let t = TmpStore::new();
    seed_graph(&t.store);

    let e = t.store.query_graph("g", "entities", None, 200).unwrap();
    let labels: Vec<&str> = e["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"Alice") && labels.contains(&"Acme") && labels.contains(&"Bob"));
    assert!(
        e.get("relations").is_none(),
        "entities view has no relations"
    );

    let r = t.store.query_graph("g", "relations", None, 200).unwrap();
    assert_eq!(r["relations"].as_array().unwrap().len(), 2);
    assert!(
        r.get("entities").is_none(),
        "relations view has no entities"
    );
}

#[test]
fn query_neighbors_returns_focal_entity_and_its_edges() {
    let t = TmpStore::new();
    seed_graph(&t.store);
    let r = t
        .store
        .query_graph("g", "neighbors", Some("Alice"), 200)
        .unwrap();
    assert_eq!(r["entity"]["label"], "Alice");
    assert_eq!(r["entity"]["description"], "An engineer");
    let out: Vec<&str> = r["outgoing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["target"].as_str().unwrap())
        .collect();
    assert!(
        out.contains(&"Acme") && out.contains(&"Bob"),
        "Alice's two edges: {out:?}"
    );

    // Acme only has an incoming edge from Alice.
    let a = t
        .store
        .query_graph("g", "neighbors", Some("Acme"), 200)
        .unwrap();
    assert_eq!(a["incoming"].as_array().unwrap().len(), 1);
    assert_eq!(a["outgoing"].as_array().unwrap().len(), 0);
    assert_eq!(a["incoming"][0]["source"], "Alice");
}

#[test]
fn query_neighbors_missing_entity_errors_with_known() {
    let t = TmpStore::new();
    seed_graph(&t.store);
    let err = t
        .store
        .query_graph("g", "neighbors", Some("Ghost"), 200)
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("'Ghost'") && msg.contains("Alice"),
        "actionable: {msg}"
    );
}

#[test]
fn query_limit_truncates_and_flags() {
    let t = TmpStore::new();
    seed_graph(&t.store);
    let r = t.store.query_graph("g", "entities", None, 2).unwrap();
    assert_eq!(r["entities"].as_array().unwrap().len(), 2);
    assert_eq!(r["truncated"], true);
}

#[test]
fn query_unknown_view_and_absent_graph_error() {
    let t = TmpStore::new();
    seed_graph(&t.store);
    assert!(t
        .store
        .query_graph("g", "bogus", None, 200)
        .unwrap_err()
        .to_string()
        .contains("unknown view"));
    assert!(t
        .store
        .query_graph("nope", "summary", None, 200)
        .unwrap_err()
        .to_string()
        .contains("no graph found"));
}

#[test]
fn concurrent_writes_to_same_path_dont_lose_updates() {
    use std::sync::Arc;
    let t = TmpStore::new();
    // One shared store (one lock) — the realistic single-server case.
    let store = Arc::new(KgStore::new(t.dir.clone()));
    let handles: Vec<_> = (0..24)
        .map(|i| {
            let s = Arc::clone(&store);
            std::thread::spawn(move || {
                s.add_entity("race", &format!("E{i}"), "other", None, HashMap::new())
                    .unwrap();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    // Without serialization, racing read-modify-write would lose most updates.
    assert_eq!(store.load("race").unwrap().entities.len(), 24);
}

#[test]
fn separate_paths_are_independent_graphs() {
    let t = TmpStore::new();
    t.store
        .add_entity("doc/one", "Alice", "person", None, HashMap::new())
        .unwrap();
    t.store
        .add_entity("doc/two", "Bob", "person", None, HashMap::new())
        .unwrap();
    assert_eq!(t.store.load("doc/one").unwrap().entities.len(), 1);
    assert_eq!(t.store.load("doc/two").unwrap().entities.len(), 1);
    assert!(t.store.resolve("doc/one").unwrap().exists());
    assert!(t.store.resolve("doc/two").unwrap().exists());
}

fn seed_schema() -> Schema {
    Schema::new(
        vec!["PERSON".into(), "ORGANIZATION".into()],
        vec!["WORKS_AT".into()],
        vec!["team".into()],
    )
}

#[test]
fn fixed_schema_rejects_values_outside_seed_schema() {
    let policy = SchemaPolicy::new(SchemaMode::Fixed, seed_schema()).unwrap();
    let t = TmpStore::with_policy(policy);

    t.store
        .add_entity("g", "Alice", "person", None, HashMap::new())
        .unwrap();
    let err = t
        .store
        .add_entity("g", "Paris", "location", None, HashMap::new())
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("fixed schema") && msg.contains("PERSON"),
        "actionable error: {msg}"
    );

    t.store
        .add_entity("g", "Acme", "organization", None, HashMap::new())
        .unwrap();
    let err = t
        .store
        .add_relation("g", "Alice", "knows", "Acme", None, None)
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("relation type") && msg.contains("WORKS_AT"),
        "relation error: {msg}"
    );

    t.store
        .add_attribute("g", "Alice", "team", serde_json::json!("platform"))
        .unwrap();
    let err = t
        .store
        .add_attribute("g", "Alice", "role", serde_json::json!("lead"))
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("attribute key") && msg.contains("team"),
        "attribute error: {msg}"
    );
}

#[test]
fn fixed_and_evolving_require_non_empty_schema() {
    assert!(SchemaPolicy::new(SchemaMode::Fixed, Schema::default()).is_err());
    assert!(SchemaPolicy::new(SchemaMode::Evolving, Schema::default()).is_err());
    assert!(SchemaPolicy::new(SchemaMode::Open, Schema::default()).is_ok());
}

#[test]
fn evolving_schema_accepts_seed_values_and_requires_proposal_for_new_values() {
    let policy = SchemaPolicy::new(SchemaMode::Evolving, seed_schema()).unwrap();
    let t = TmpStore::with_policy(policy);

    t.store
        .add_entity("g", "Alice", "person", None, HashMap::new())
        .unwrap();
    let err = t
        .store
        .add_entity("g", "Dune", "WORK_OF_ART", None, HashMap::new())
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("propose_schema_type") && msg.contains("WORK_OF_ART"),
        "{msg}"
    );

    t.store
        .propose_schema_type(
            "g",
            "node",
            "WORK_OF_ART",
            Some("Document discusses films".into()),
        )
        .unwrap();
    t.store
        .add_entity("g", "Dune", "WORK_OF_ART", None, HashMap::new())
        .unwrap();

    t.store
        .propose_schema_type(
            "g",
            "relation",
            "INSPIRED_BY",
            Some("Needed by text".into()),
        )
        .unwrap();
    t.store
        .add_relation("g", "Dune", "INSPIRED_BY", "Alice", None, None)
        .unwrap();

    let kg = t.store.load("g").unwrap();
    assert_eq!(kg.metadata["schema_mode"], serde_json::json!("evolving"));
    assert_eq!(
        kg.metadata["new_schema_types"]["nodes"][0],
        serde_json::json!("WORK_OF_ART")
    );
    assert_eq!(
        kg.metadata["new_schema_types"]["relations"][0],
        serde_json::json!("INSPIRED_BY")
    );
    assert!(kg.metadata.contains_key("schema_used"));
    assert_eq!(kg.triples.len(), 1);
}

#[test]
fn propose_schema_type_is_only_available_in_evolving_mode() {
    let t = TmpStore::new();
    let err = t
        .store
        .propose_schema_type("g", "node", "MOVIE", None)
        .unwrap_err();
    assert!(err.to_string().contains("evolving schema mode"));
}

#[test]
fn query_schema_reports_policy_and_path_proposals() {
    let policy = SchemaPolicy::new(SchemaMode::Evolving, seed_schema()).unwrap();
    let t = TmpStore::with_policy(policy);
    t.store
        .propose_schema_type("g", "attribute", "box_office", None)
        .unwrap();

    let r = t.store.query_schema("g").unwrap();
    assert_eq!(r["schema_mode"], serde_json::json!("evolving"));
    assert_eq!(r["schema"]["nodes"][0], serde_json::json!("PERSON"));
    assert_eq!(
        r["new_schema_types"]["attributes"][0],
        serde_json::json!("box_office")
    );
}
