use super::*;
use crate::backend::MockBackend;
use crate::types::{Entity, EntityType, Predicate, PredicateType, Triple};

/// One entity carrying a description, then triples whose endpoint
/// snapshots are the same (rich) entities so `add_triple`'s endpoint
/// upsert does not clobber the descriptions.
fn entity(id: &str, desc: &str) -> Entity {
    let mut e = Entity::new(id, id, EntityType::Other);
    e.description = Some(desc.into());
    e
}

fn triple_rich(subj: &Entity, obj: &Entity) -> Triple {
    Triple::new(
        subj.clone(),
        Predicate::new(PredicateType::RelatedTo),
        obj.clone(),
    )
}

/// Two disjoint triangles {a,b,c} and {x,y,z}, all described.
fn two_triangles() -> (KnowledgeGraph, Vec<Entity>) {
    let entities: Vec<Entity> = ["a", "b", "c", "x", "y", "z"]
        .into_iter()
        .map(|n| entity(n, &format!("description of {n}")))
        .collect();
    let mut kg = KnowledgeGraph::new();
    let by = |n: &str| entities.iter().find(|e| e.id == n).unwrap().clone();
    for (s, o) in [
        ("a", "b"),
        ("b", "c"),
        ("c", "a"),
        ("x", "y"),
        ("y", "z"),
        ("z", "x"),
    ] {
        kg.add_triple(triple_rich(&by(s), &by(o)));
    }
    (kg, entities)
}

/// A backend whose every call fails — exercises the degradation path.
struct FailingBackend;

#[async_trait::async_trait]
impl LlmBackend for FailingBackend {
    async fn complete(
        &self,
        _messages: &[Message],
        _options: &CompletionOptions,
    ) -> anyhow::Result<String> {
        anyhow::bail!("backend down")
    }
}

/// Fails only the prompt that lists member "a" — partial degradation
/// under concurrency: the other community must still be summarized.
struct PartialFailBackend;

#[async_trait::async_trait]
impl LlmBackend for PartialFailBackend {
    async fn complete(
        &self,
        messages: &[Message],
        _options: &CompletionOptions,
    ) -> anyhow::Result<String> {
        let prompt = messages.last().map(|m| m.content.as_str()).unwrap_or("");
        if prompt.contains("- a (") {
            anyhow::bail!("backend down for community a");
        }
        Ok(r#"{"name": "ok", "summary": "fine."}"#.to_string())
    }
}

/// Concurrency probe: replies are tied to the prompt (echoing the first
/// listed member id, so result attribution is observable in the output),
/// calls yield a per-prompt number of times so concurrent calls complete
/// **out of order** (the "a" community is issued first but finishes
/// last), and the max simultaneous in-flight count is recorded to prove
/// the concurrency cap.
struct OverlapBackend {
    in_flight: std::sync::atomic::AtomicUsize,
    max_seen: Arc<std::sync::atomic::AtomicUsize>,
}

impl OverlapBackend {
    fn new() -> (Arc<Self>, Arc<std::sync::atomic::AtomicUsize>) {
        let max_seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (
            Arc::new(Self {
                in_flight: std::sync::atomic::AtomicUsize::new(0),
                max_seen: max_seen.clone(),
            }),
            max_seen,
        )
    }
}

#[async_trait::async_trait]
impl LlmBackend for OverlapBackend {
    async fn complete(
        &self,
        messages: &[Message],
        _options: &CompletionOptions,
    ) -> anyhow::Result<String> {
        use std::sync::atomic::Ordering;
        let n = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_seen.fetch_max(n, Ordering::SeqCst);
        let prompt = messages.last().map(|m| m.content.clone()).unwrap_or_default();
        // Out-of-order completion: the first-issued community ("a")
        // yields more, so it finishes after the later-issued one.
        let yields = if prompt.contains("- a (") { 6 } else { 1 };
        for _ in 0..yields {
            tokio::task::yield_now().await;
        }
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        let first_member = prompt
            .lines()
            .find_map(|l| l.strip_prefix("- "))
            .and_then(|l| l.split(" (").next())
            .unwrap_or("?")
            .to_string();
        Ok(format!(
            r#"{{"name": "group of {first_member}", "summary": "members around {first_member}."}}"#
        ))
    }
}

#[tokio::test]
async fn summaries_render_as_objects_with_name_and_summary() {
    let (kg, _) = two_triangles();
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::single(
        r#"{"name": "Triangle Report", "summary": "Three tightly linked nodes."}"#,
    ));
    let value = communities_json_with_summaries(&kg, &backend, &CompletionOptions::default(), 8).await;
    let communities = value["communities"].as_object().unwrap();
    assert_eq!(communities.len(), 2);
    for c in communities.values() {
        assert_eq!(c["name"], "Triangle Report");
        assert_eq!(c["summary"], "Three tightly linked nodes.");
        assert_eq!(c["members"].as_array().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn summary_prompts_and_output_are_deterministic() {
    let (kg, _) = two_triangles();
    let mock = || Arc::new(MockBackend::single(r#"{"name": "N", "summary": "S."}"#));
    let opts = CompletionOptions::default();
    let first = communities_json_with_summaries(&kg, &(mock() as Arc<dyn LlmBackend>), &opts, 8).await;
    let second = communities_json_with_summaries(&kg, &(mock() as Arc<dyn LlmBackend>), &opts, 8).await;
    assert_eq!(first, second, "same graph + same replies → identical JSON");

    // One call per community, issued in ascending community-label order:
    // the first prompt must list the members of community "0".
    let backend = mock();
    let value = communities_json_with_summaries(
        &kg,
        &(backend.clone() as Arc<dyn LlmBackend>),
        &opts,
        8,
    )
    .await;
    let prompts = backend.seen_prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2, "one completion per community");
    let community_zero: Vec<&str> = value["communities"]["0"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    for id in community_zero {
        assert!(
            prompts[0].contains(&format!("- {id} (OTHER)")),
            "first prompt must cover community 0 member {id}"
        );
    }
}

#[tokio::test]
async fn concurrent_output_is_byte_identical_to_sequential() {
    let (kg, _) = two_triangles();
    let opts = CompletionOptions::default();
    let (serial_backend, _) = OverlapBackend::new();
    let serial =
        communities_json_with_summaries(&kg, &(serial_backend as Arc<dyn LlmBackend>), &opts, 1)
            .await;
    let (conc_backend, _) = OverlapBackend::new();
    let concurrent =
        communities_json_with_summaries(&kg, &(conc_backend as Arc<dyn LlmBackend>), &opts, 8)
            .await;
    assert_eq!(
        serde_json::to_string(&serial).unwrap(),
        serde_json::to_string(&concurrent).unwrap(),
        "out-of-order completion must not change the output bytes"
    );
    // Attribution: each community's reply echoes its own first member even
    // though the "a" community's call completed after the "x" one.
    for c in concurrent["communities"].as_object().unwrap().values() {
        let first_member = c["members"][0].as_str().unwrap();
        assert_eq!(c["name"], format!("group of {first_member}"));
    }
}

#[tokio::test]
async fn concurrency_cap_bounds_in_flight_calls() {
    use std::sync::atomic::Ordering;
    let (kg, _) = two_triangles();
    let opts = CompletionOptions::default();
    let (backend, max_seen) = OverlapBackend::new();
    let _ = communities_json_with_summaries(&kg, &(backend as Arc<dyn LlmBackend>), &opts, 2).await;
    assert_eq!(
        max_seen.load(Ordering::SeqCst),
        2,
        "cap 2 lets both community calls overlap"
    );
    let (backend, max_seen) = OverlapBackend::new();
    let _ = communities_json_with_summaries(&kg, &(backend as Arc<dyn LlmBackend>), &opts, 1).await;
    assert_eq!(
        max_seen.load(Ordering::SeqCst),
        1,
        "cap 1 keeps the calls sequential"
    );
}

#[tokio::test]
async fn concurrent_partial_failure_degrades_only_that_community() {
    let (kg, _) = two_triangles();
    let backend: Arc<dyn LlmBackend> = Arc::new(PartialFailBackend);
    let value = communities_json_with_summaries(&kg, &backend, &CompletionOptions::default(), 8).await;
    for c in value["communities"].as_object().unwrap().values() {
        let has_a = c["members"].as_array().unwrap().iter().any(|m| m == "a");
        if has_a {
            assert!(c["name"].is_null(), "failed community degrades to null");
            assert!(c["summary"].is_null());
        } else {
            assert_eq!(c["name"], "ok", "unaffected community is summarized");
            assert_eq!(c["summary"], "fine.");
        }
    }
}

#[tokio::test]
async fn failing_backend_degrades_to_null_fields() {
    let (kg, _) = two_triangles();
    let backend: Arc<dyn LlmBackend> = Arc::new(FailingBackend);
    let value = communities_json_with_summaries(&kg, &backend, &CompletionOptions::default(), 8).await;
    let communities = value["communities"].as_object().unwrap();
    assert_eq!(communities.len(), 2, "degradation keeps every community");
    for c in communities.values() {
        assert!(c["name"].is_null());
        assert!(c["summary"].is_null());
        assert_eq!(c["members"].as_array().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn unparseable_reply_degrades_to_null_fields() {
    let (kg, _) = two_triangles();
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::single("no json here"));
    let value = communities_json_with_summaries(&kg, &backend, &CompletionOptions::default(), 8).await;
    for c in value["communities"].as_object().unwrap().values() {
        assert!(c["name"].is_null());
        assert!(c["summary"].is_null());
    }
}

#[test]
fn prompt_truncates_members_triples_and_descriptions() {
    // 40-node chain: 40 members (> 32) and 39 triples (> 24).
    let long_desc = "d".repeat(500);
    let entities: Vec<Entity> = (0..40).map(|i| entity(&format!("n{i}"), &long_desc)).collect();
    let mut kg = KnowledgeGraph::new();
    for w in entities.windows(2) {
        kg.add_triple(triple_rich(&w[0], &w[1]));
    }
    let member_ids: Vec<String> = entities.iter().map(|e| e.id.clone()).collect();
    let prompt = community_prompt(&kg, &member_ids);
    assert!(prompt.contains("(+8 more entities)"), "member overflow is noted");
    assert!(
        prompt.contains("(+15 more relationships)"),
        "triple overflow is noted"
    );
    assert!(
        !prompt.contains(&"d".repeat(200)),
        "descriptions are truncated to {SUMMARY_MAX_DESC_CHARS} chars"
    );
    // Deterministic: same input → same prompt.
    assert_eq!(prompt, community_prompt(&kg, &member_ids));
}
