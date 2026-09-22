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
#[path = "community_tests.rs"]
mod tests;


#[cfg(test)]
#[path = "community_summary_tests.rs"]
mod summary_tests;


#[cfg(all(test, feature = "community-leiden"))]
#[path = "community_leiden_tests.rs"]
mod leiden_tests;

