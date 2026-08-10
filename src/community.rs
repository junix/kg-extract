//! Community detection over an extracted [`KnowledgeGraph`] (ADR-987 §D#2).
//!
//! Closes the "orphan library" gap: `kg-community` shipped 5 clean detectors
//! behind a `CommunityDetector` trait but nothing wired the extraction output
//! into it. This adapter maps a `KnowledgeGraph` (entities keyed by id, triples
//! as edges) onto `kg_community::Graph` (contiguous node indices + edge list),
//! runs a detector, and maps the resulting `Partition` labels back to entity ids.
//!
//! Enabled by the optional `community` feature.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::stream::{self, StreamExt};
use kg_community::{CommunityDetector, Graph, LabelPropagation, Partition};

#[cfg(feature = "community-leiden")]
use kg_community::HierarchicalLeiden;

use crate::backend::{CompletionOptions, LlmBackend, Message};
use crate::json::extract_json_from_response;
use crate::types::KnowledgeGraph;

/// Build a [`kg_community::Graph`] from a [`KnowledgeGraph`].
///
/// Entities become nodes indexed by insertion order; each triple becomes one
/// undirected edge of weight `1.0` between its subject and object. Edges are
/// **not** deduplicated: `kg_community::Graph` keeps parallel edges and the
/// detectors sum them, so N triples between the same pair of entities act as
/// an edge of weight N — KG edge multiplicity is a genuine community-strength
/// signal and MUST NOT be flattened away (that is what the old
/// `Graph::from_edges` path did). Self-loops and triples whose endpoints are
/// not registered entities are skipped. Returns the id list (index `i` ↔
/// entity id) so `Partition` labels can be mapped back.
pub fn to_community_graph(kg: &KnowledgeGraph) -> (Vec<String>, Graph) {
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    let mut ids: Vec<String> = Vec::with_capacity(kg.entities.len());
    for (id, _entity) in kg.entities.iter() {
        index.entry(id.as_str()).or_insert_with(|| {
            ids.push(id.clone());
            ids.len() - 1
        });
    }

    let mut graph = Graph::new(index.len());
    for triple in &kg.triples {
        if let (Some(&s), Some(&o)) = (
            index.get(triple.subject.id.as_str()),
            index.get(triple.object.id.as_str()),
        ) {
            graph.add_weighted_edge(s, o, 1.0);
        }
    }

    (ids, graph)
}

/// Detect communities with a caller-supplied detector, returning a stable
/// `entity_id -> community label` map.
pub fn detect_communities<D: CommunityDetector>(
    kg: &KnowledgeGraph,
    detector: &D,
) -> BTreeMap<String, usize> {
    let (ids, graph) = to_community_graph(kg);
    let partition: Partition = detector.detect(&graph);
    ids.into_iter()
        .enumerate()
        .map(|(i, id)| (id, partition.community_of(i)))
        .collect()
}

/// Convenience: detect with the dependency-free [`LabelPropagation`] detector.
pub fn detect_communities_label_propagation(kg: &KnowledgeGraph) -> BTreeMap<String, usize> {
    detect_communities(kg, &LabelPropagation::default())
}

/// Render one [`Partition`] level as the shared JSON shape:
/// `{num_communities, quality, communities: {"0": [entity_id, …], …}}`.
/// `quality` is the engine-reported score (`null` when the detector does not
/// provide one — e.g. label propagation). Community keys ascend from `"0"`;
/// member ids are sorted, so the output is deterministic across runs.
///
/// When `summaries` is supplied (the `--community-summaries` path), each
/// community renders as an object `{members, name, summary}` instead of a
/// bare id array; `name`/`summary` are `null` for a community whose LLM call
/// failed or was unparseable (degradation, never a hard error).
fn partition_json(
    ids: &[String],
    partition: &Partition,
    summaries: Option<&BTreeMap<usize, CommunitySummary>>,
) -> serde_json::Value {
    let mut members: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (i, id) in ids.iter().enumerate() {
        members
            .entry(partition.community_of(i))
            .or_default()
            .push(id.clone());
    }
    let communities: serde_json::Map<String, serde_json::Value> = members
        .into_iter()
        .map(|(c, mut ids)| {
            ids.sort();
            let value = match summaries {
                Some(map) => {
                    let summary = map.get(&c).cloned().unwrap_or_default();
                    serde_json::json!({
                        "members": ids,
                        "name": summary.name,
                        "summary": summary.summary,
                    })
                }
                None => serde_json::Value::Array(
                    ids.into_iter().map(serde_json::Value::String).collect(),
                ),
            };
            (c.to_string(), value)
        })
        .collect();
    serde_json::json!({
        "num_communities": communities.len(),
        "quality": partition.quality(),
        "communities": communities,
    })
}

/// Render communities as a JSON document for the CLI's `communities` output
/// format: `{num_communities, quality, communities: {"0": [entity_id, …], …}}`.
/// Detection uses the dependency-free [`LabelPropagation`] detector over the
/// multiplicity-weighted graph (see [`to_community_graph`]), which reports no
/// quality score, so `quality` is `null`. Community keys ascend from `"0"`;
/// member ids are sorted, so the output is deterministic across runs.
pub fn communities_json(kg: &KnowledgeGraph) -> serde_json::Value {
    let (ids, graph) = to_community_graph(kg);
    let partition = LabelPropagation::default().detect(&graph);
    partition_json(&ids, &partition, None)
}

/// Fixed seed for the CLI's hierarchical Leiden run so repeated runs over the
/// same graph emit byte-identical JSON (Leiden's local moves are randomised).
#[cfg(feature = "community-leiden")]
const HIERARCHY_SEED: u64 = 42;

/// Render hierarchical Leiden communities for the CLI's
/// `communities-hierarchy` output format:
/// `{detector, num_levels, levels: [{level, quality, num_communities,
/// communities}]}`. Levels are ordered **coarse → fine** (`level` 0 is the
/// root aggregation round and matches the grouping of a flat Leiden run);
/// each level carries its modularity quality score. Detection runs with a
/// fixed seed ([`HIERARCHY_SEED`]) and member ids are sorted, so the output
/// is deterministic across runs.
///
/// Enabled by the `community-leiden` feature.
#[cfg(feature = "community-leiden")]
pub fn hierarchy_json(kg: &KnowledgeGraph) -> serde_json::Value {
    let (ids, graph) = to_community_graph(kg);
    let levels = HierarchicalLeiden::new()
        .seed(HIERARCHY_SEED)
        .detect_hierarchy(&graph);
    let levels_json: Vec<serde_json::Value> = levels
        .iter()
        .enumerate()
        .map(|(level, partition)| {
            let mut value = partition_json(&ids, partition, None);
            value["level"] = serde_json::Value::from(level);
            value
        })
        .collect();
    serde_json::json!({
        "detector": "hierarchical-leiden",
        "num_levels": levels.len(),
        "levels": levels_json,
    })
}

// ---------------------------------------------------------------------------
// Community summaries (GraphRAG-style reports)
// ---------------------------------------------------------------------------

/// LLM-written report for one community: a short `name` plus a prose
/// `summary`. `None` fields mean the community was **degraded** — the backend
/// call failed or returned something unparseable — and the community ships
/// without a summary rather than failing the run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommunitySummary {
    pub name: Option<String>,
    pub summary: Option<String>,
}

/// Cap on member entities listed in one summary prompt. GraphRAG communities
/// can be arbitrarily large; past this many members the marginal signal for
/// naming the community is small while the token cost grows linearly, so the
/// prompt lists the first N (sorted ids) and notes the remainder.
pub const SUMMARY_MAX_MEMBERS: usize = 32;

/// Cap on intra-community relationships listed in one summary prompt, same
/// rationale as [`SUMMARY_MAX_MEMBERS`].
pub const SUMMARY_MAX_TRIPLES: usize = 24;

/// Per-entity description budget (chars) inside the prompt: entity
/// descriptions are model-written prose and can run long; one community has
/// dozens of them, so each is truncated to keep the prompt bounded.
pub const SUMMARY_MAX_DESC_CHARS: usize = 120;

/// `max_tokens` for a summary completion: the reply is one small JSON object
/// (`{"name", "summary"}`), so 1024 leaves generous headroom while bounding
/// cost per community.
pub const SUMMARY_MAX_TOKENS: u32 = 1024;

/// Char-boundary-safe truncation with an ellipsis marker.
fn truncate_chars(s: &str, max: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(max).collect();
    format!("{cut}…")
}

/// Build the summary prompt for one community: its member entities (label +
/// type + truncated description) and its intra-community triples. Deterministic
/// — member ids arrive sorted and the triple lines are sorted — so the same
/// graph always produces the same prompt.
fn community_prompt(kg: &KnowledgeGraph, member_ids: &[String]) -> String {
    let mut entity_lines: Vec<String> = Vec::new();
    for id in member_ids.iter().take(SUMMARY_MAX_MEMBERS) {
        if let Some(e) = kg.entities.get(id.as_str()) {
            let mut line = format!("- {} ({})", e.label, e.output_type());
            let desc = e
                .description
                .as_deref()
                .map(|d| truncate_chars(d, SUMMARY_MAX_DESC_CHARS))
                .unwrap_or_default();
            if !desc.is_empty() {
                line.push_str(": ");
                line.push_str(&desc);
            }
            entity_lines.push(line);
        }
    }
    if member_ids.len() > SUMMARY_MAX_MEMBERS {
        entity_lines.push(format!(
            "- (+{} more entities)",
            member_ids.len() - SUMMARY_MAX_MEMBERS
        ));
    }

    let member_set: std::collections::HashSet<&str> =
        member_ids.iter().map(String::as_str).collect();
    let mut triple_lines: Vec<String> = kg
        .triples
        .iter()
        .filter(|t| {
            member_set.contains(t.subject.id.as_str())
                && member_set.contains(t.object.id.as_str())
        })
        .map(|t| {
            format!(
                "- {} — {} → {}",
                t.subject.label,
                t.predicate.display_label(),
                t.object.label
            )
        })
        .collect();
    triple_lines.sort();
    let total_triples = triple_lines.len();
    triple_lines.truncate(SUMMARY_MAX_TRIPLES);
    if total_triples > SUMMARY_MAX_TRIPLES {
        triple_lines.push(format!(
            "- (+{} more relationships)",
            total_triples - SUMMARY_MAX_TRIPLES
        ));
    }

    format!(
        "You are given one community of a knowledge graph. Write a short report for it.\n\
         \n\
         Entities:\n{}\n\
         \n\
         Relationships:\n{}\n\
         \n\
         Reply with ONLY a JSON object of the form \
         {{\"name\": \"...\", \"summary\": \"...\"}} (no prose, no code fence):\n\
         - \"name\": a concise title for the community (a few words)\n\
         - \"summary\": 2-4 sentences describing what ties these entities together\n\
         Write both in the language of the entity labels (English when they are English).",
        entity_lines.join("\n"),
        triple_lines.join("\n"),
    )
}

/// Parse the model's reply into a [`CommunitySummary`]. Accepts the reply via
/// the shared JSON extraction (fence, then whole string); blank/missing
/// fields stay `None`.
fn parse_summary_response(text: &str) -> CommunitySummary {
    let clean = |v: Option<&serde_json::Value>| {
        v.and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
    };
    match extract_json_from_response(text) {
        Some(v) => CommunitySummary {
            name: clean(v.get("name")),
            summary: clean(v.get("summary")),
        },
        None => CommunitySummary::default(),
    }
}

/// Generate a [`CommunitySummary`] for every community of one partition.
///
/// Backend calls run with **bounded concurrency**: up to `max_concurrency`
/// (clamped to ≥ 1) completions are in flight at once, issued in ascending
/// community-label order with sorted members so every prompt is deterministic.
/// The determinism contract is on the **output**, not the call sequence: each
/// result is keyed by its own community label and `buffered` yields in issue
/// order, so completion order never leaks into the returned map — given the
/// same backend replies, the map (and the rendered JSON) is byte-identical to
/// a sequential run. A failed or unparseable call degrades that community to
/// `CommunitySummary::default()` (null `name`/`summary` in the JSON) with a
/// stderr warning naming the community — the spec's silent-degradation error
/// model — instead of failing the run. (Warning lines may interleave in
/// completion order; stderr is not part of the output contract.)
pub async fn summarize_partition(
    kg: &KnowledgeGraph,
    ids: &[String],
    partition: &Partition,
    backend: &Arc<dyn LlmBackend>,
    options: &CompletionOptions,
    max_concurrency: usize,
) -> BTreeMap<usize, CommunitySummary> {
    let mut members: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (i, id) in ids.iter().enumerate() {
        members
            .entry(partition.community_of(i))
            .or_default()
            .push(id.clone());
    }
    // LLM calls are I/O-bound, so `buffered` runs up to `max_conc` in flight
    // while preserving issue (ascending-label) order — same pattern as the
    // Simple engine's per-chunk concurrency.
    let max_conc = max_concurrency.max(1);
    stream::iter(members)
        .map(|(c, mut ids)| async move {
            ids.sort();
            let prompt = community_prompt(kg, &ids);
            let summary = match backend.complete(&[Message::user(prompt)], options).await {
                Ok(text) => {
                    let parsed = parse_summary_response(&text);
                    if parsed.name.is_none() && parsed.summary.is_none() {
                        eprintln!(
                            "warning: community summary for community {c} was unparseable; \
                             emitting null name/summary"
                        );
                    }
                    parsed
                }
                Err(e) => {
                    eprintln!(
                        "warning: community summary for community {c} failed: {e}; \
                         emitting null name/summary"
                    );
                    CommunitySummary::default()
                }
            };
            (c, summary)
        })
        .buffered(max_conc)
        .collect()
        .await
}

/// [`communities_json`] plus per-community LLM reports: each community
/// renders as `{"members": [entity_id, …], "name": …, "summary": …}` with
/// `null` fields for degraded communities. Summary calls run with bounded
/// concurrency ([`summarize_partition`]); the output is deterministic.
pub async fn communities_json_with_summaries(
    kg: &KnowledgeGraph,
    backend: &Arc<dyn LlmBackend>,
    options: &CompletionOptions,
    max_concurrency: usize,
) -> serde_json::Value {
    let (ids, graph) = to_community_graph(kg);
    let partition = LabelPropagation::default().detect(&graph);
    let summaries =
        summarize_partition(kg, &ids, &partition, backend, options, max_concurrency).await;
    partition_json(&ids, &partition, Some(&summaries))
}

/// [`hierarchy_json`] plus per-community LLM reports at **every level**
/// (coarse → fine): each level's communities render as `{members, name,
/// summary}` objects, with `null` fields for degraded communities. Levels are
/// processed sequentially in coarse→fine order; within one level the summary
/// calls run with bounded concurrency ([`summarize_partition`]). The output
/// is deterministic.
///
/// Enabled by the `community-leiden` feature.
#[cfg(feature = "community-leiden")]
pub async fn hierarchy_json_with_summaries(
    kg: &KnowledgeGraph,
    backend: &Arc<dyn LlmBackend>,
    options: &CompletionOptions,
    max_concurrency: usize,
) -> serde_json::Value {
    let (ids, graph) = to_community_graph(kg);
    let levels = HierarchicalLeiden::new()
        .seed(HIERARCHY_SEED)
        .detect_hierarchy(&graph);
    let mut levels_json: Vec<serde_json::Value> = Vec::with_capacity(levels.len());
    for (level, partition) in levels.iter().enumerate() {
        let summaries =
            summarize_partition(kg, &ids, partition, backend, options, max_concurrency).await;
        let mut value = partition_json(&ids, partition, Some(&summaries));
        value["level"] = serde_json::Value::from(level);
        levels_json.push(value);
    }
    serde_json::json!({
        "detector": "hierarchical-leiden",
        "num_levels": levels.len(),
        "levels": levels_json,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Entity, EntityType, Predicate, PredicateType, Triple};

    fn triple(subj: &str, obj: &str) -> Triple {
        Triple::new(
            Entity::new(subj, subj, EntityType::Other),
            Predicate::new(PredicateType::RelatedTo),
            Entity::new(obj, obj, EntityType::Other),
        )
    }

    #[test]
    fn maps_entities_and_triples_onto_a_community_graph() {
        let mut kg = KnowledgeGraph::new();
        // Two disjoint triangles: {a,b,c} and {x,y,z}.
        for (s, o) in [
            ("a", "b"),
            ("b", "c"),
            ("c", "a"),
            ("x", "y"),
            ("y", "z"),
            ("z", "x"),
        ] {
            kg.add_triple(triple(s, o));
        }
        let (ids, graph) = to_community_graph(&kg);
        assert_eq!(ids.len(), 6);
        assert_eq!(graph.num_nodes(), 6);
        assert_eq!(graph.edges().len(), 6);
    }

    #[test]
    fn multiplicity_produces_parallel_weighted_edges() {
        // Three triples between the same pair must NOT collapse into one edge:
        // kg-community sums parallel edges, so multiplicity becomes weight.
        let mut kg = KnowledgeGraph::new();
        for _ in 0..3 {
            kg.add_triple(triple("a", "b"));
        }
        kg.add_triple(triple("b", "c"));

        let (ids, graph) = to_community_graph(&kg);
        let pos = |id: &str| ids.iter().position(|i| i == id).unwrap();
        let (a, b, c) = (pos("a"), pos("b"), pos("c"));
        assert_eq!(graph.edges().len(), 4, "every triple keeps its own edge");
        let weight_ab: f64 = graph
            .edges()
            .iter()
            .filter(|&&(s, o, _)| (s == a && o == b) || (s == b && o == a))
            .map(|&(_, _, w)| w)
            .sum();
        assert_eq!(weight_ab, 3.0, "multiplicity is preserved as summed weight");
        let weight_bc: f64 = graph
            .edges()
            .iter()
            .filter(|&&(s, o, _)| (s == b && o == c) || (s == c && o == b))
            .map(|&(_, _, w)| w)
            .sum();
        assert_eq!(weight_bc, 1.0);
    }

    #[test]
    fn canonical_direction_merges_direction_variant_multiplicity() {
        // (a, USES, b) and (b, IS_USED_BY, a) are the same semantic edge in two
        // directions. After canonical-direction normalisation + merge dedup
        // they are ONE triple, so the community graph sees one edge of weight
        // 1.0 — while a genuinely distinct edge (a, PART_OF, b) keeps its own
        // weight, so the pair's summed multiplicity lands at 2.0 (not 3.0).
        use crate::merger::{merge_with_deduplication, normalize_direction};

        let ent = |id: &str| Entity::new(id, id, EntityType::Other);
        let directed = |s: &str, p: PredicateType, o: &str| {
            Triple::new(ent(s), Predicate::new(p), ent(o))
        };

        let mut g1 = KnowledgeGraph::new();
        g1.add_triple(directed("a", PredicateType::Uses, "b"));
        let mut g2 = KnowledgeGraph::new();
        g2.add_triple(directed("b", PredicateType::IsUsedBy, "a"));
        g2.add_triple(directed("a", PredicateType::PartOf, "b"));
        normalize_direction(&mut g1);
        normalize_direction(&mut g2);

        let kg = merge_with_deduplication(g1, g2);
        assert_eq!(kg.triples.len(), 2, "direction variants collapse to one edge");

        let (ids, graph) = to_community_graph(&kg);
        let pos = |id: &str| ids.iter().position(|i| i == id).unwrap();
        let (a, b) = (pos("a"), pos("b"));
        assert_eq!(graph.edges().len(), 2);
        let weight_ab: f64 = graph
            .edges()
            .iter()
            .filter(|&&(s, o, _)| (s == a && o == b) || (s == b && o == a))
            .map(|&(_, _, w)| w)
            .sum();
        assert_eq!(
            weight_ab, 2.0,
            "merged variant (1.0) + distinct PART_OF edge (1.0), not 3.0"
        );
    }

    #[test]
    fn multiplicity_weights_change_the_partition() {
        // n1–n2 is a heavy pair (5 triples); n0–n1, n2–n3, n0–n3 are single
        // triples. Summed multiplicity must bind n1 to n2 (and n0 to n3),
        // yielding TWO communities; the old dedup adapter (every edge weight
        // 1.0) collapses the whole component into one.
        let mut kg = KnowledgeGraph::new();
        kg.add_triple(triple("n0", "n1"));
        kg.add_triple(triple("n2", "n3"));
        for _ in 0..5 {
            kg.add_triple(triple("n1", "n2"));
        }
        kg.add_triple(triple("n0", "n3"));
        let labels = detect_communities_label_propagation(&kg);
        assert_eq!(labels["n1"], labels["n2"], "the heavy pair binds");
        assert_eq!(labels["n0"], labels["n3"]);
        assert_ne!(
            labels["n0"], labels["n1"],
            "multiplicity splits the component where a flat graph would not"
        );
    }

    #[test]
    fn communities_json_groups_members_by_community() {
        let mut kg = KnowledgeGraph::new();
        for (s, o) in [
            ("a", "b"),
            ("b", "c"),
            ("c", "a"),
            ("x", "y"),
            ("y", "z"),
            ("z", "x"),
        ] {
            kg.add_triple(triple(s, o));
        }
        let value = communities_json(&kg);
        assert_eq!(value["num_communities"], 2);
        assert!(
            value["quality"].is_null(),
            "label propagation reports no quality score"
        );
        let communities = value["communities"].as_object().unwrap();
        assert_eq!(communities.len(), 2);
        let mut groups: Vec<Vec<String>> = communities
            .values()
            .map(|v| {
                v.as_array()
                    .unwrap()
                    .iter()
                    .map(|id| id.as_str().unwrap().to_string())
                    .collect()
            })
            .collect();
        for g in &mut groups {
            g.sort();
        }
        groups.sort();
        assert_eq!(
            groups,
            vec![
                vec!["a".to_string(), "b".to_string(), "c".to_string()],
                vec!["x".to_string(), "y".to_string(), "z".to_string()],
            ]
        );
    }

    #[test]
    fn detects_two_disjoint_clusters() {
        let mut kg = KnowledgeGraph::new();
        for (s, o) in [
            ("a", "b"),
            ("b", "c"),
            ("c", "a"),
            ("x", "y"),
            ("y", "z"),
            ("z", "x"),
        ] {
            kg.add_triple(triple(s, o));
        }
        let labels = detect_communities_label_propagation(&kg);
        assert_eq!(labels.len(), 6);
        // Same triangle → same community; different triangles → different.
        assert_eq!(labels["a"], labels["b"]);
        assert_eq!(labels["b"], labels["c"]);
        assert_eq!(labels["x"], labels["y"]);
        assert_ne!(labels["a"], labels["x"]);
    }
}

#[cfg(test)]
mod summary_tests {
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
}

#[cfg(all(test, feature = "community-leiden"))]
mod leiden_tests {
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
}
