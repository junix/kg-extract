//! Knowledge-graph merging utilities (ported from `graph/kg_extractor/merger.py`).
//!
//! Duplicate entities (same lowercased label) are combined per a
//! [`MergeStrategy`]: `KeepExisting` (the historical default) discards the
//! incoming copy; `KeepIncoming` / `FieldUnion` preserve information from both;
//! `Llm` asks the model to synthesise a merged description (async — see
//! [`merge_knowledge_graphs_llm`]).

use crate::backend::{CompletionOptions, LlmBackend};
use crate::types::{CorefMode, Entity, EntityType, KnowledgeGraph, MergeStrategy, Triple};
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;

/// Minimum normalised length for the fuzzy (edit-distance) coref path. Short
/// names ("Go", "C", "AI") are far too collision-prone to match by similarity,
/// so they only ever merge via exact or exact-normalised equality.
const FUZZY_MIN_LEN: usize = 6;

/// Similarity (1 − normalised edit distance) at or above which two normalised
/// labels are treated as the same entity. Tuned so a single-character typo in a
/// name of [`FUZZY_MIN_LEN`]+ characters fuses, while distinct short names stay
/// apart; combined with the type-compatibility gate, coref errs toward missing
/// rather than wrongly fusing two distinct entities.
const FUZZY_SIMILARITY_THRESHOLD: f64 = 0.85;

/// Corporate / legal suffix tokens stripped when normalising a label, so
/// `"Anthropic"`, `"Anthropic PBC"` and `"Anthropic, Inc."` collapse to one.
#[rustfmt::skip]
const CORP_SUFFIXES: &[&str] = &[
    "inc", "incorporated", "llc", "ltd", "limited", "corp", "corporation", "co", "company", "plc",
    "pbc", "gmbh", "ag", "sa", "nv", "oy", "ab", "srl", "spa", "pty", "llp", "group", "holdings",
];

/// Leading articles stripped when normalising a multi-token label.
const LEADING_ARTICLES: &[&str] = &["the", "a", "an"];

/// Normalise an entity label for coreference: lowercase, fold every
/// non-alphanumeric run to a single space, drop a leading article, then strip
/// trailing corporate-suffix tokens. Returns the canonical token string (which
/// may be empty for a label of pure punctuation).
fn normalize_label(label: &str) -> String {
    let lowered = label.to_lowercase();
    let spaced: String = lowered
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let mut tokens: Vec<&str> = spaced.split_whitespace().collect();
    if tokens.len() > 1 && LEADING_ARTICLES.contains(&tokens[0]) {
        tokens.remove(0);
    }
    while tokens.len() > 1 && CORP_SUFFIXES.contains(tokens.last().unwrap()) {
        tokens.pop();
    }
    tokens.join(" ")
}

/// Classic Levenshtein edit distance over Unicode scalar values.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Normalised similarity in `[0, 1]`: `1 − dist / max_len`.
fn similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let max = a.chars().count().max(b.chars().count());
    if max == 0 {
        return 1.0;
    }
    1.0 - levenshtein(a, b) as f64 / max as f64
}

/// Two entity types may denote the same entity if they agree, or if either is
/// the generic `Other` (a chunk that failed to type the entity must still be
/// allowed to fuse with one that typed it).
fn types_compatible(a: EntityType, b: EntityType) -> bool {
    a == b || a == EntityType::Other || b == EntityType::Other
}

/// Index of the entities already in the accumulator, used to decide whether an
/// incoming entity is a duplicate. Always tracks exact lowercased labels; when
/// `coref` is [`CorefMode::Fuzzy`] it also tracks normalised labels and keeps an
/// insertion-ordered list for the edit-distance fallback (so resolution is
/// deterministic — ties go to the earliest-inserted entity).
struct DedupIndex {
    label_to_id: HashMap<String, String>,
    norm_to_id: HashMap<String, String>,
    norms: Vec<(String, String)>,
    coref: CorefMode,
}

impl DedupIndex {
    fn seed(g1: &KnowledgeGraph, coref: CorefMode) -> Self {
        let mut idx = DedupIndex {
            label_to_id: HashMap::new(),
            norm_to_id: HashMap::new(),
            norms: Vec::new(),
            coref,
        };
        for (id, e) in g1.entities.iter() {
            idx.register(&e.label, id);
        }
        idx
    }

    /// Record a newly-added entity so later incoming entities can match it.
    fn register(&mut self, label: &str, id: &str) {
        self.label_to_id
            .entry(label.to_lowercase())
            .or_insert_with(|| id.to_string());
        if self.coref != CorefMode::Off {
            let n = normalize_label(label);
            if !n.is_empty() {
                self.norm_to_id
                    .entry(n.clone())
                    .or_insert_with(|| id.to_string());
                self.norms.push((n, id.to_string()));
            }
        }
    }

    /// The id of an already-present entity judged identical to `entity`, or
    /// `None`. Exact lowercased label first; then (fuzzy only) exact-normalised,
    /// then a type-compatible edit-distance match above the threshold.
    fn lookup(&self, g1: &KnowledgeGraph, entity: &Entity) -> Option<String> {
        if let Some(id) = self.label_to_id.get(&entity.label.to_lowercase()) {
            return Some(id.clone());
        }
        if self.coref == CorefMode::Off {
            return None;
        }
        let n = normalize_label(&entity.label);
        if n.is_empty() {
            return None;
        }
        if let Some(id) = self.norm_to_id.get(&n) {
            return Some(id.clone());
        }
        if n.chars().count() < FUZZY_MIN_LEN {
            return None;
        }
        let mut best: Option<(f64, &str)> = None;
        for (cand, id) in &self.norms {
            if cand.chars().count() < FUZZY_MIN_LEN {
                continue;
            }
            let compatible = g1
                .entities
                .get(id)
                .is_none_or(|e| types_compatible(e.entity_type, entity.entity_type));
            if !compatible {
                continue;
            }
            let sim = similarity(&n, cand);
            // `>` (not `>=`) keeps the earliest-inserted entity on ties, so the
            // result is independent of HashMap iteration order.
            if sim >= FUZZY_SIMILARITY_THRESHOLD && best.is_none_or(|(b, _)| sim > b) {
                best = Some((sim, id));
            }
        }
        best.map(|(_, id)| id.to_string())
    }
}

/// Merge `graph2` into `graph1` with the historical `KeepExisting` strategy.
/// When `merge_duplicates`, entities are deduplicated by lowercased label;
/// otherwise a plain id-based merge is used.
pub fn merge_knowledge_graphs(
    graph1: KnowledgeGraph,
    graph2: KnowledgeGraph,
    merge_duplicates: bool,
) -> KnowledgeGraph {
    merge_knowledge_graphs_with(
        graph1,
        graph2,
        merge_duplicates,
        MergeStrategy::KeepExisting,
    )
}

/// Merge `graph2` into `graph1`, combining label-duplicates per `strategy`.
/// `MergeStrategy::Llm` degrades to `FieldUnion` here (no backend); use
/// [`merge_knowledge_graphs_llm`] for the backend-backed variant.
pub fn merge_knowledge_graphs_with(
    mut graph1: KnowledgeGraph,
    graph2: KnowledgeGraph,
    merge_duplicates: bool,
    strategy: MergeStrategy,
) -> KnowledgeGraph {
    if merge_duplicates {
        merge_with_deduplication_strategy(graph1, graph2, strategy)
    } else {
        graph1.merge(graph2);
        graph1
    }
}

/// Merge two graphs deduplicating entities by `label.lower()` with the historical
/// `KeepExisting` strategy, remapping the second graph's triples.
pub fn merge_with_deduplication(g1: KnowledgeGraph, g2: KnowledgeGraph) -> KnowledgeGraph {
    merge_with_deduplication_strategy(g1, g2, MergeStrategy::KeepExisting)
}

/// Dedup entities by `label.lower()`, combining each collision per `strategy`
/// (synchronously — `Llm` behaves as `FieldUnion`). Remaps g2's triples and
/// drops duplicate triples.
pub fn merge_with_deduplication_strategy(
    g1: KnowledgeGraph,
    g2: KnowledgeGraph,
    strategy: MergeStrategy,
) -> KnowledgeGraph {
    merge_with_deduplication_strategy_coref(g1, g2, strategy, CorefMode::Off)
}

/// Like [`merge_with_deduplication_strategy`], but `coref` controls how
/// duplicates are *recognised*: [`CorefMode::Off`] matches exact lowercased
/// labels only; [`CorefMode::Fuzzy`] also collapses normalised / near-identical,
/// type-compatible labels — the cross-chunk coreference path.
pub fn merge_with_deduplication_strategy_coref(
    mut g1: KnowledgeGraph,
    g2: KnowledgeGraph,
    strategy: MergeStrategy,
    coref: CorefMode,
) -> KnowledgeGraph {
    let mut index = DedupIndex::seed(&g1, coref);
    // id in g2 -> id in merged g1.
    let mut id_mapping: HashMap<String, String> = HashMap::new();

    for (id, entity) in g2.entities.iter() {
        if let Some(existing_id) = index.lookup(&g1, entity) {
            // Same entity → combine into the one already present (KeepExisting
            // leaves it untouched, preserving the historical behaviour exactly —
            // except provenance, which always survives from both sides).
            if strategy != MergeStrategy::KeepExisting {
                if let Some(existing) = g1.entities.get(&existing_id).cloned() {
                    let merged = combine_entities(strategy, &existing, entity, None);
                    g1.entities.insert(existing_id.clone(), merged);
                }
            } else if let Some(existing) = g1.entities.get_mut(&existing_id) {
                crate::citation::union_citations(&mut existing.metadata, &entity.metadata);
            }
            id_mapping.insert(id.clone(), existing_id);
        } else {
            add_nonmatching_entity(&mut g1, &mut index, &mut id_mapping, id, entity);
        }
    }

    remap_triples(&mut g1, g2.triples, &id_mapping);
    g1
}

/// Async LLM-aware variant of [`merge_with_deduplication_strategy`]: on a label
/// collision whose two descriptions are both non-empty and differ, the model is
/// asked to synthesise one merged description (falling back to `FieldUnion`).
pub async fn merge_knowledge_graphs_llm(
    mut g1: KnowledgeGraph,
    g2: KnowledgeGraph,
    backend: &Arc<dyn LlmBackend>,
    opts: &CompletionOptions,
) -> KnowledgeGraph {
    let mut index = DedupIndex::seed(&g1, CorefMode::Off);
    let mut id_mapping: HashMap<String, String> = HashMap::new();

    for (id, entity) in g2.entities.iter() {
        if let Some(existing_id) = index.lookup(&g1, entity) {
            if let Some(existing) = g1.entities.get(&existing_id).cloned() {
                let llm_desc = synthesize_description(backend, opts, &existing, entity).await;
                let merged = combine_entities(MergeStrategy::Llm, &existing, entity, llm_desc);
                g1.entities.insert(existing_id.clone(), merged);
            }
            id_mapping.insert(id.clone(), existing_id);
        } else {
            add_nonmatching_entity(&mut g1, &mut index, &mut id_mapping, id, entity);
        }
    }

    remap_triples(&mut g1, g2.triples, &id_mapping);
    g1
}

/// Fold a list of graphs left-to-right with LLM-aware dedup (mirrors
/// [`merge_all`], but applying the `Llm` strategy via `backend`).
pub async fn merge_all_dedup_llm(
    graphs: Vec<KnowledgeGraph>,
    backend: &Arc<dyn LlmBackend>,
    opts: &CompletionOptions,
) -> KnowledgeGraph {
    let mut acc = KnowledgeGraph::new();
    for g in graphs {
        acc = merge_knowledge_graphs_llm(acc, g, backend, opts).await;
    }
    acc
}

/// Self-dedup a freshly built graph per `strategy` (dedup `kg` against an empty
/// graph). Uses `backend` only when the strategy needs it (`Llm`), so non-LLM
/// strategies never touch the network despite the async signature.
pub async fn dedup_graph(
    kg: KnowledgeGraph,
    strategy: MergeStrategy,
    backend: &Arc<dyn LlmBackend>,
    opts: &CompletionOptions,
) -> KnowledgeGraph {
    dedup_graph_coref(kg, strategy, CorefMode::Off, backend, opts).await
}

/// Like [`dedup_graph`], but `coref` controls duplicate recognition. The `Llm`
/// strategy still matches exact labels only (its async per-collision synthesis
/// is orthogonal to fuzzy recognition); every other strategy honours `coref`.
pub async fn dedup_graph_coref(
    kg: KnowledgeGraph,
    strategy: MergeStrategy,
    coref: CorefMode,
    backend: &Arc<dyn LlmBackend>,
    opts: &CompletionOptions,
) -> KnowledgeGraph {
    if strategy.needs_backend() {
        merge_knowledge_graphs_llm(KnowledgeGraph::new(), kg, backend, opts).await
    } else {
        merge_with_deduplication_strategy_coref(KnowledgeGraph::new(), kg, strategy, coref)
    }
}

/// Combine two entities judged identical (same lowercased label). `llm_desc`
/// (set only for a synthesised LLM description) overrides the description; the
/// result always keeps `existing.id` so the table key and `entity.id` agree.
pub fn combine_entities(
    strategy: MergeStrategy,
    existing: &Entity,
    incoming: &Entity,
    llm_desc: Option<String>,
) -> Entity {
    #[allow(unused_mut)]
    let mut merged = match strategy {
        MergeStrategy::KeepExisting => existing.clone(),
        MergeStrategy::KeepIncoming => {
            let mut e = incoming.clone();
            e.id = existing.id.clone();
            e
        }
        MergeStrategy::FieldUnion | MergeStrategy::Llm => {
            let mut e = existing.clone();
            e.confidence = match (existing.confidence, incoming.confidence) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            e.description = llm_desc
                .filter(|s| !s.trim().is_empty())
                .or_else(|| richer_description(&existing.description, &incoming.description));
            // A specific type wins over a generic `Other`.
            if existing.entity_type == EntityType::Other
                && incoming.entity_type != EntityType::Other
            {
                e.entity_type = incoming.entity_type;
            }
            if e.raw_type.is_none() {
                e.raw_type = incoming.raw_type.clone();
            }
            // Union metadata; existing keys win on conflict.
            for (k, v) in &incoming.metadata {
                e.metadata.entry(k.clone()).or_insert_with(|| v.clone());
            }
            e
        }
    };
    // Whatever the strategy keeps, provenance from BOTH sides survives: the
    // merged entity was seen everywhere either duplicate was seen.
    {
        crate::citation::union_citations(&mut merged.metadata, &existing.metadata);
        crate::citation::union_citations(&mut merged.metadata, &incoming.metadata);
    }
    merged
}

/// The longer (more informative) of two optional descriptions.
fn richer_description(a: &Option<String>, b: &Option<String>) -> Option<String> {
    match (a.as_deref(), b.as_deref()) {
        (Some(x), Some(y)) => {
            let pick = if y.trim().chars().count() > x.trim().chars().count() {
                y
            } else {
                x
            };
            Some(pick.to_string())
        }
        (Some(x), None) => Some(x.to_string()),
        (None, Some(y)) => Some(y.to_string()),
        (None, None) => None,
    }
}

/// Ask the LLM to merge two differing descriptions of the same entity. Returns
/// `None` (so the caller falls back to `FieldUnion`) when either side is empty,
/// the two are identical, or the call fails.
async fn synthesize_description(
    backend: &Arc<dyn LlmBackend>,
    opts: &CompletionOptions,
    existing: &Entity,
    incoming: &Entity,
) -> Option<String> {
    let a = existing.description.as_deref().map(str::trim).unwrap_or("");
    let b = incoming.description.as_deref().map(str::trim).unwrap_or("");
    if a.is_empty() || b.is_empty() || a == b {
        return None;
    }
    let prompt = format!(
        "Two descriptions refer to the same entity \"{}\". Merge them into ONE concise, \
         non-repetitive description that preserves every distinct fact. Output only the merged \
         description, with no preamble.\n\nA: {a}\n\nB: {b}",
        existing.label
    );
    match backend.complete_prompt(&prompt, opts).await {
        Ok(s) => {
            let s = s.trim().to_string();
            (!s.is_empty()).then_some(s)
        }
        Err(_) => None,
    }
}

/// Add a g2 entity that has no duplicate in g1: rewrite its id on a key
/// collision (mints `[N+1]`), else insert under its own id. Registers the new
/// entity in `index` (so later g2 entities can dedup against it) and records the
/// id remap.
fn add_nonmatching_entity(
    g1: &mut KnowledgeGraph,
    index: &mut DedupIndex,
    id_mapping: &mut HashMap<String, String>,
    id: &str,
    entity: &Entity,
) {
    if g1.entities.contains_key(id) {
        // ID collision with a different label → fresh id. The entity's own `.id`
        // must be rewritten to the new key, otherwise the stored entity (and
        // every triple endpoint cloned from it) keeps the colliding old id.
        let new_id = generate_new_id(g1);
        let mut e = entity.clone();
        e.id = new_id.clone();
        id_mapping.insert(id.to_string(), new_id.clone());
        g1.entities.insert(new_id.clone(), e);
        index.register(&entity.label, &new_id);
    } else {
        g1.entities.insert(id.to_string(), entity.clone());
        index.register(&entity.label, id);
        id_mapping.insert(id.to_string(), id.to_string());
    }
}

/// Remap g2's triples through `id_mapping`, binding endpoints to g1's (merged)
/// entity snapshots and dropping duplicates / dangling endpoints.
fn remap_triples(
    g1: &mut KnowledgeGraph,
    g2_triples: Vec<Triple>,
    id_mapping: &HashMap<String, String>,
) {
    // key -> index into g1.triples, so a dropped duplicate can still hand its
    // provenance to the kept triple.
    let mut existing: HashMap<(String, String, String), usize> = g1
        .triples
        .iter()
        .enumerate()
        .map(|(i, t)| (t.to_tuple(), i))
        .collect();
    for triple in g2_triples {
        let subj_id = id_mapping
            .get(&triple.subject.id)
            .cloned()
            .unwrap_or_else(|| triple.subject.id.clone());
        let obj_id = id_mapping
            .get(&triple.object.id)
            .cloned()
            .unwrap_or_else(|| triple.object.id.clone());
        let (Some(subj), Some(obj)) = (
            g1.entities.get(&subj_id).cloned(),
            g1.entities.get(&obj_id).cloned(),
        ) else {
            continue;
        };
        let remapped = Triple {
            subject: subj,
            predicate: triple.predicate,
            object: obj,
            confidence: triple.confidence,
            metadata: triple.metadata,
        };
        let key = remapped.to_tuple();
        match existing.get(&key) {
            Some(&_kept_idx) => {
                crate::citation::union_citations(
                    &mut g1.triples[_kept_idx].metadata,
                    &remapped.metadata,
                );
            }
            None => {
                existing.insert(key, g1.triples.len());
                g1.triples.push(remapped);
            }
        }
    }
}

/// Merge a list of graphs left-to-right with triple dedup (mirrors
/// `BaseExtractor.merge_knowledge_graphs`).
pub fn merge_all(graphs: Vec<KnowledgeGraph>) -> KnowledgeGraph {
    let mut iter = graphs.into_iter();
    let Some(mut acc) = iter.next() else {
        return KnowledgeGraph::new();
    };
    for g in iter {
        acc.merge(g);
    }
    acc
}

/// Fold a list of graphs left-to-right with `strategy` dedup and `coref`
/// duplicate-recognition (the synchronous, non-LLM cross-chunk merge path).
pub fn merge_all_dedup_coref(
    graphs: Vec<KnowledgeGraph>,
    strategy: MergeStrategy,
    coref: CorefMode,
) -> KnowledgeGraph {
    graphs.into_iter().fold(KnowledgeGraph::new(), |acc, g| {
        merge_with_deduplication_strategy_coref(acc, g, strategy, coref)
    })
}

/// Generate `[N+1]`, where N is the max integer found in any existing id.
pub fn generate_new_id(graph: &KnowledgeGraph) -> String {
    let re = Regex::new(r"\d+").unwrap();
    let mut max_id = 0i64;
    for id in graph.entities.keys() {
        if let Some(m) = re.find(id) {
            if let Ok(n) = m.as_str().parse::<i64>() {
                max_id = max_id.max(n);
            }
        }
    }
    format!("[{}]", max_id + 1)
}

/// Find entities with an identical (case-insensitive) label.
pub fn find_similar_entities(entity: &Entity, entities: &[Entity]) -> Vec<String> {
    let target = entity.label.to_lowercase();
    entities
        .iter()
        .filter(|e| e.id != entity.id && e.label.to_lowercase() == target)
        .map(|e| e.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EntityType, Predicate, PredicateType};

    #[test]
    fn dedup_collision_rewrites_entity_id_to_new_key() {
        // g1 has "e1"=Alice; g2 reuses "e1" for a different entity (Bob) plus a
        // Bob->Paris relation. Bob must land under a fresh key whose value's `.id`
        // equals that key, and the remapped triple must carry the fresh id — not
        // the colliding "e1" that belongs to Alice.
        let mut g1 = KnowledgeGraph::new();
        g1.add_entity(Entity::new("e1", "Alice", EntityType::Person));

        let bob = Entity::new("e1", "Bob", EntityType::Person);
        let paris = Entity::new("e3", "Paris", EntityType::City);
        let mut g2 = KnowledgeGraph::new();
        g2.add_entity(bob.clone());
        g2.add_entity(paris.clone());
        g2.add_triple(Triple::new(
            bob.clone(),
            Predicate::new(PredicateType::LocatedIn),
            paris,
        ));

        let merged = merge_with_deduplication(g1, g2);

        // Table key and entity.id must agree for every entity.
        for (key, e) in merged.entities.iter() {
            assert_eq!(
                &e.id, key,
                "entity '{}' stored under key '{}' has stale id",
                e.label, key
            );
        }
        let bob_key = merged
            .entities
            .iter()
            .find(|(_, e)| e.label == "Bob")
            .map(|(k, _)| k.clone())
            .expect("Bob present");
        assert_ne!(bob_key, "e1", "Bob must get a fresh id, not Alice's e1");
        // The Bob->Paris triple endpoint must carry Bob's fresh id.
        let t = merged
            .triples
            .iter()
            .find(|t| t.subject.label == "Bob")
            .expect("Bob relation kept");
        assert_eq!(
            t.subject.id, bob_key,
            "triple subject id must match Bob's table key"
        );
    }

    #[test]
    fn field_union_combines_description_confidence_metadata_and_type() {
        let mut a = Entity::new("e1", "Acme", EntityType::Organization);
        a.description = Some("short".into());
        a.confidence = Some(0.5);
        a.metadata.insert("x".into(), serde_json::json!(1));
        let mut b = Entity::new("e2", "acme", EntityType::Other);
        b.description = Some("a much longer, richer description".into());
        b.confidence = Some(0.9);
        b.metadata.insert("y".into(), serde_json::json!(2));

        let merged = combine_entities(MergeStrategy::FieldUnion, &a, &b, None);
        assert_eq!(merged.id, "e1", "must keep the canonical (existing) id");
        assert_eq!(
            merged.description.as_deref(),
            Some("a much longer, richer description")
        );
        assert_eq!(
            merged.confidence,
            Some(0.9),
            "confidence is the max of both"
        );
        assert!(merged.metadata.contains_key("x") && merged.metadata.contains_key("y"));
        assert_eq!(
            merged.entity_type,
            EntityType::Organization,
            "specific type beats Other"
        );
    }

    #[test]
    fn keep_incoming_replaces_but_preserves_canonical_id() {
        let a = Entity::new("e1", "Acme", EntityType::Organization);
        let mut b = Entity::new("e2", "Acme", EntityType::City);
        b.description = Some("new".into());
        let merged = combine_entities(MergeStrategy::KeepIncoming, &a, &b, None);
        assert_eq!(merged.id, "e1");
        assert_eq!(merged.entity_type, EntityType::City);
        assert_eq!(merged.description.as_deref(), Some("new"));
    }

    #[test]
    fn field_union_dedup_keeps_the_richer_description() {
        let mut g1 = KnowledgeGraph::new();
        let mut a = Entity::new("e1", "Acme", EntityType::Organization);
        a.description = Some("HQ in NY".into());
        g1.add_entity(a);
        let mut g2 = KnowledgeGraph::new();
        let mut b = Entity::new("e9", "acme", EntityType::Organization);
        b.description = Some("a global manufacturing company".into());
        g2.add_entity(b);

        let merged = merge_with_deduplication_strategy(g1, g2, MergeStrategy::FieldUnion);
        assert_eq!(merged.entities.len(), 1, "same-label entities collapse");
        let e = merged.entities.values().next().unwrap();
        assert_eq!(
            e.description.as_deref(),
            Some("a global manufacturing company")
        );
    }

    #[test]
    fn normalize_label_strips_punctuation_articles_and_suffixes() {
        assert_eq!(normalize_label("OpenAI"), "openai");
        assert_eq!(normalize_label("Open AI"), "open ai");
        assert_eq!(normalize_label("Anthropic, PBC"), "anthropic");
        assert_eq!(normalize_label("Google Inc."), "google");
        assert_eq!(normalize_label("The New York Times"), "new york times");
        // A bare suffix token is never stripped to empty.
        assert_eq!(normalize_label("Inc"), "inc");
    }

    #[test]
    fn coref_off_keeps_surface_variants_separate() {
        let mut g1 = KnowledgeGraph::new();
        g1.add_entity(Entity::new("e1", "Anthropic", EntityType::Company));
        let mut g2 = KnowledgeGraph::new();
        g2.add_entity(Entity::new("e2", "Anthropic, PBC", EntityType::Company));

        let merged = merge_with_deduplication_strategy_coref(
            g1,
            g2,
            MergeStrategy::FieldUnion,
            CorefMode::Off,
        );
        assert_eq!(merged.entities.len(), 2, "Off mode must not fuse variants");
    }

    #[test]
    fn coref_fuzzy_merges_normalized_variant_and_remaps_triples() {
        // g1: "Anthropic" founded by a person. g2 calls it "Anthropic, PBC" and
        // adds another relation. Fuzzy coref must collapse the two company nodes
        // and rebind every triple endpoint onto the surviving id.
        let anthropic = Entity::new("e1", "Anthropic", EntityType::Company);
        let dario = Entity::new("e2", "Dario", EntityType::Person);
        let mut g1 = KnowledgeGraph::new();
        g1.add_entity(anthropic.clone());
        g1.add_entity(dario.clone());
        g1.add_triple(Triple::new(
            anthropic,
            Predicate::new(PredicateType::FoundedBy),
            dario,
        ));

        let anthropic2 = Entity::new("e9", "Anthropic, PBC", EntityType::Company);
        let claude = Entity::new("e8", "Claude", EntityType::Technology);
        let mut g2 = KnowledgeGraph::new();
        g2.add_entity(anthropic2.clone());
        g2.add_entity(claude.clone());
        g2.add_triple(Triple::new(
            anthropic2,
            Predicate::new(PredicateType::DevelopedBy),
            claude,
        ));

        let merged = merge_with_deduplication_strategy_coref(
            g1,
            g2,
            MergeStrategy::FieldUnion,
            CorefMode::Fuzzy,
        );
        // Anthropic(+PBC) collapse → 3 entities: Anthropic, Dario, Claude.
        assert_eq!(
            merged.entities.len(),
            3,
            "the two Anthropic nodes must fuse"
        );
        assert!(merged.entities.contains_key("e1"), "canonical id survives");
        // Both relations now reference the canonical Anthropic id e1. (DevelopedBy
        // swaps actor→artifact, so Anthropic is the object there.)
        assert_eq!(merged.triples.len(), 2);
        for t in &merged.triples {
            assert!(
                t.subject.id == "e1" || t.object.id == "e1",
                "every triple must touch the canonical Anthropic id"
            );
        }
    }

    #[test]
    fn coref_fuzzy_merges_near_typo_but_respects_type_and_length() {
        // Long near-identical, type-compatible labels fuse...
        let mut g1 = KnowledgeGraph::new();
        g1.add_entity(Entity::new("e1", "Anthropic", EntityType::Company));
        let mut g2 = KnowledgeGraph::new();
        g2.add_entity(Entity::new("e2", "Antropic", EntityType::Company));
        let merged = merge_with_deduplication_strategy_coref(
            g1,
            g2,
            MergeStrategy::FieldUnion,
            CorefMode::Fuzzy,
        );
        assert_eq!(merged.entities.len(), 1, "near-typo of a long name fuses");

        // ...but short labels never fuzzy-fuse (too collision-prone).
        let mut g3 = KnowledgeGraph::new();
        g3.add_entity(Entity::new("e1", "Go", EntityType::Technology));
        let mut g4 = KnowledgeGraph::new();
        g4.add_entity(Entity::new("e2", "Rust", EntityType::Technology));
        let merged = merge_with_deduplication_strategy_coref(
            g3,
            g4,
            MergeStrategy::FieldUnion,
            CorefMode::Fuzzy,
        );
        assert_eq!(merged.entities.len(), 2, "short labels stay distinct");
    }

    #[tokio::test]
    async fn llm_strategy_synthesizes_merged_description() {
        use crate::backend::MockBackend;
        let synthesized = "Acme: a global manufacturer headquartered in NY.";
        let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::single(synthesized));
        let opts = CompletionOptions::default();

        let mut g1 = KnowledgeGraph::new();
        let mut a = Entity::new("e1", "Acme", EntityType::Organization);
        a.description = Some("HQ in NY".into());
        g1.add_entity(a);
        let mut g2 = KnowledgeGraph::new();
        let mut b = Entity::new("e2", "Acme", EntityType::Organization);
        b.description = Some("global manufacturer".into());
        g2.add_entity(b);

        let merged = merge_knowledge_graphs_llm(g1, g2, &backend, &opts).await;
        assert_eq!(merged.entities.len(), 1);
        let e = merged.entities.values().next().unwrap();
        assert_eq!(e.description.as_deref(), Some(synthesized));
    }
}
