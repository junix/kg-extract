use super::*;
use crate::types::{Entity, EntityType, Predicate, PredicateType, Triple};

/// Zachary karate club (34 nodes, 78 undirected edges) as triples — the
/// canonical benchmark that aggregates over multiple Leiden levels.
fn karate_kg() -> KnowledgeGraph {
    const EDGES: &[(usize, usize)] = &[
        (0, 1), (0, 2), (0, 3), (0, 4), (0, 5), (0, 6), (0, 7), (0, 8), (0, 10), (0, 11),
        (0, 12), (0, 13), (0, 17), (0, 19), (0, 21), (0, 31), (1, 2), (1, 3), (1, 7), (1, 13),
        (1, 17), (1, 19), (1, 21), (1, 30), (2, 3), (2, 7), (2, 8), (2, 9), (2, 13), (2, 27),
        (2, 28), (2, 32), (3, 7), (3, 12), (3, 13), (4, 6), (4, 10), (5, 6), (5, 10), (5, 16),
        (6, 16), (8, 30), (8, 32), (8, 33), (9, 33), (13, 33), (14, 32), (14, 33), (15, 32),
        (15, 33), (18, 32), (18, 33), (19, 33), (20, 32), (20, 33), (22, 32), (22, 33),
        (23, 25), (23, 27), (23, 29), (23, 32), (23, 33), (24, 25), (24, 27), (24, 31),
        (25, 31), (26, 29), (26, 33), (27, 33), (28, 31), (28, 33), (29, 32), (29, 33),
        (30, 32), (30, 33), (31, 32), (31, 33), (32, 33),
    ];
    let mut kg = KnowledgeGraph::new();
    for &(s, o) in EDGES {
        let (s, o) = (format!("n{s}"), format!("n{o}"));
        kg.add_triple(Triple::new(
            Entity::new(s.clone(), s, EntityType::Other),
            Predicate::new(PredicateType::RelatedTo),
            Entity::new(o.clone(), o, EntityType::Other),
        ));
    }
    kg
}

fn community_of<'a>(level: &'a serde_json::Value, id: &str) -> &'a str {
    level["communities"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, members)| members.as_array().unwrap().iter().any(|m| m == id))
        .map(|(c, _)| c.as_str())
        .unwrap_or_else(|| panic!("{id} not covered by level"))
}

#[test]
fn hierarchy_json_layers_coarse_to_fine_with_quality() {
    let value = hierarchy_json(&karate_kg());
    assert_eq!(value["detector"], "hierarchical-leiden");
    let levels = value["levels"].as_array().unwrap();
    assert_eq!(value["num_levels"], levels.len());
    assert!(levels.len() > 1, "karate should aggregate over levels");

    // Level indices are sequential from 0; community counts are
    // non-decreasing (coarse → fine); every level carries a finite
    // quality score and covers all 34 entity ids with sorted members.
    let mut prev_count = 0;
    for (i, level) in levels.iter().enumerate() {
        assert_eq!(level["level"], i);
        let quality = level["quality"]
            .as_f64()
            .expect("Leiden reports modularity per level");
        assert!(quality.is_finite(), "quality must be finite, got {quality}");
        let communities = level["communities"].as_object().unwrap();
        assert_eq!(level["num_communities"], communities.len());
        assert!(
            communities.len() >= prev_count,
            "levels not coarse→fine: {} < {}",
            communities.len(),
            prev_count
        );
        prev_count = communities.len();
        let covered: usize = communities
            .values()
            .map(|m| m.as_array().unwrap().len())
            .sum();
        assert_eq!(covered, 34, "every level covers every entity");
        for members in communities.values() {
            let ids: Vec<&str> = members
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m.as_str().unwrap())
                .collect();
            let mut sorted = ids.clone();
            sorted.sort_unstable();
            assert_eq!(ids, sorted, "member ids must be sorted");
        }
    }

    // The root level splits the two karate factions (leaders n0 / n33).
    assert_ne!(
        community_of(&levels[0], "n0"),
        community_of(&levels[0], "n33"),
        "root level must separate the karate leaders"
    );
}

#[test]
fn hierarchy_json_is_deterministic_across_runs() {
    let kg = karate_kg();
    assert_eq!(hierarchy_json(&kg), hierarchy_json(&kg));
}

#[tokio::test]
async fn hierarchy_summaries_cover_every_level_and_stay_deterministic() {
    use crate::backend::{CompletionOptions, LlmBackend, MockBackend};
    use std::sync::Arc;

    let kg = karate_kg();
    let opts = CompletionOptions::default();
    let mk = || {
        Arc::new(MockBackend::single(r#"{"name": "Faction", "summary": "A karate faction."}"#))
            as Arc<dyn LlmBackend>
    };
    let value = hierarchy_json_with_summaries(&kg, &mk(), &opts, 8).await;
    let levels = value["levels"].as_array().unwrap();
    assert!(levels.len() > 1);
    let mut total_communities = 0;
    for level in levels {
        for c in level["communities"].as_object().unwrap().values() {
            total_communities += 1;
            assert_eq!(c["name"], "Faction", "every level's community named");
            assert_eq!(c["summary"], "A karate faction.");
            assert!(!c["members"].as_array().unwrap().is_empty());
        }
    }

    // One completion per (level, community), in level order then ascending
    // community-label order; identical across runs.
    let backend = mk();
    let again = hierarchy_json_with_summaries(&kg, &backend, &opts, 8).await;
    assert_eq!(value, again);
    let backend = Arc::new(MockBackend::single(r#"{"name": "F", "summary": "S."}"#));
    let _ = hierarchy_json_with_summaries(&kg, &(backend.clone() as Arc<dyn LlmBackend>), &opts, 8).await;
    assert_eq!(
        backend.seen_prompts.lock().unwrap().len(),
        total_communities,
        "one completion per community per level"
    );
}
