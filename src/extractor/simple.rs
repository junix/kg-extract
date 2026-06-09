//! SimpleExtractor — general LLM chat extractor with GraphRAG-style delimiter
//! prompting and multi-gleaning (ported from `graph/kg_extractor/simple.py`).

use super::{validate_input, Extractor};
use crate::backend::{ChatSession, CompletionOptions, LlmBackend, ReplaySession};
use crate::chunking::segment;
use crate::graph_build::entity_id;
use crate::merger::{merge_all, merge_all_dedup_llm, merge_knowledge_graphs_with};
use crate::types::{
    Entity, EntityType, ExtractionConfig, ExtractionResponse, KnowledgeGraph, ParsedResult,
    Predicate, PredicateType, Triple,
};
use futures::stream::{self, StreamExt};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub const MAX_GLEANINGS: usize = 2;
const TUPLE_DELIMITER: &str = "<|>";
const RECORD_DELIMITER: &str = "##";

const EXTRACTION_PROMPT: &str = r#"
--- GOAL ---
Given a text document and a schema definition, identify all entities, relationships, and attributes from the text according to the schema.

--- SCHEMA ---
Entity Types: {entity_types}
Relationship Types: {relationship_types}
Attribute Types: {attribute_types}

--- STEPS ---
1. Identify all entities. For each identified entity, extract the following information:
- entity_name: Name of the entity, capitalized, in language of 'Text'
- entity_type: One of the following types: {entity_types}
- entity_description: IMPORTANT - Provide a detailed and comprehensive description (at least 10-20 words) of the entity explaining what it is, its significance, and its key characteristics. DO NOT LEAVE THIS EMPTY.
- entity_attributes: Key attributes of the entity (if any from the attribute types: {attribute_types})
Format each entity as (entity<|>entity_name<|>entity_type<|>entity_description<|>entity_attributes)

2. From the entities identified in step 1, identify all pairs of (source_entity, target_entity) that are *clearly related* to each other.
For each pair of related entities, extract the following information:
- source_entity: name of the source entity, as identified in step 1
- target_entity: name of the target entity, as identified in step 1
- relationship_type: type of relationship from the schema (one of: {relationship_types})
- relationship_description: explanation as to why you think the source entity and the target entity are related to each other in language of 'Text'
- relationship_strength: a numeric score indicating strength of the relationship between the source entity and target entity
Format each relationship as (relationship<|>source_entity<|>target_entity<|>relationship_type<|>relationship_description<|>relationship_strength)

3. Return output as a single list of all the entities and relationships identified in steps 1 and 2. Use `##` as the list delimiter.
4. One entity or relation must use `<|>` as the tuple delimiter.

-- EXAMPLE OUTPUT --
```text
(entity<|>DeepSeek-R1-Distill-Qwen-14B<|>model<|>A 14-billion parameter model used in reinforcement learning experiments.<|>type: language model, parameters: 14B)##
(relationship<|>DeepSeek-R1-Distill-Qwen-14B<|>GRPO<|>uses<|>The model uses GRPO algorithm for optimization during training.<|>0.9)##
```

--- REAL DATA ---
Context about the text:
<text-context>
{context}
</text-context>
Text:
<chunk-to-analysis>
{chunk}
</chunk-to-analysis>
Output:
"#;

const CONTINUE_PROMPT: &str = "Seem MANY entities/relations were missed in the extraction. If there are no more entities/relations that need to be added, just answer NO. Otherwise, add them below using the same format:\n";

/// System prompt shared by every turn of a chunk's extraction session.
const SYSTEM_PROMPT: &str = "You are an elite assistant for extracting structured data from text.";

/// Targeted *relation* gleaning. Plain entity gleaning (`CONTINUE_PROMPT`) asks
/// "what did you miss?" and the model tends to answer with more *nouns*, leaving
/// freshly-named entities dangling. This prompt does the opposite: it names the
/// entities that ended up with **no** relationship and asks specifically *why* —
/// connect each to the graph, or explicitly leave it alone. Endpoints are
/// resolved against the already-extracted entities, so the round only adds edges
/// (optionally pulling in a new neighbour the model names to anchor one).
///
/// Sent as a follow-up *within the same session*, so the source text is already
/// in context (native multi-turn) or replayed in the history (`ReplaySession`) —
/// the chunk is intentionally not re-embedded here.
const RELATION_GLEANING_PROMPT: &str = r#"
Looking back at the text you just analysed: the entities below were extracted but
have NO relationship linking them to anything. For EACH one:
- if it is clearly related to another entity (one from this list, or any other
  entity in the text), emit that relationship;
- if it genuinely stands alone in this text, skip it — do NOT invent a link.

Orphan entities:
{orphans}

Relationship Types to prefer: {relationship_types}

Output ONLY relationships, `##`-separated, each in EXACTLY this format:
(relationship<|>source_entity<|>target_entity<|>relationship_type<|>why they are related<|>strength_0_to_1)

If none of these entities has any relationship in the text, answer just: NO
Output:
"#;

/// Knowledge graph extractor using a general LLM chat interface.
pub struct SimpleExtractor {
    backend: Arc<dyn LlmBackend>,
    config: ExtractionConfig,
    pub quiet: bool,
    pub max_gleanings: usize,
    /// Targeted relation-gleaning rounds run *after* entity gleaning: each round
    /// re-questions the chunk's orphan entities (no incident edge) to recover
    /// their relationships. `0` (default) preserves the Python port's behaviour.
    pub max_relation_gleanings: usize,
    pub context: String,
}

impl SimpleExtractor {
    /// Default config: `qwen-max`, segment_size 5000.
    pub fn default_config() -> ExtractionConfig {
        ExtractionConfig {
            model_name: "qwen-max".into(),
            segment_size: 5000,
            min_segment_size: 100,
            ..Default::default()
        }
    }

    pub fn new(backend: Arc<dyn LlmBackend>) -> Self {
        SimpleExtractor {
            backend,
            config: Self::default_config(),
            quiet: false,
            max_gleanings: MAX_GLEANINGS,
            max_relation_gleanings: 0,
            context: String::new(),
        }
    }

    pub fn with_config(backend: Arc<dyn LlmBackend>, config: ExtractionConfig) -> Self {
        SimpleExtractor {
            backend,
            config,
            quiet: false,
            max_gleanings: MAX_GLEANINGS,
            max_relation_gleanings: 0,
            context: String::new(),
        }
    }

    /// Enable targeted relation-gleaning with `n` rescue rounds (builder style).
    pub fn relation_gleanings(mut self, n: usize) -> Self {
        self.max_relation_gleanings = n;
        self
    }

    pub fn config(&self) -> &ExtractionConfig {
        &self.config
    }

    /// Extract one chunk via the GraphRAG prompt + multi-gleaning loop, building
    /// that chunk's (un-deduped) graph. Multiple chunks run this concurrently.
    async fn extract_chunk(&self, chunk: &str) -> (Vec<ParsedResult>, KnowledgeGraph) {
        let entity_types = self.config.entity_types_list().join(", ");
        let rel_types = if self.config.predicates_list().is_empty() {
            "related_to, part_of, uses".to_string()
        } else {
            self.config.predicates_list().join(", ")
        };
        let attr_types = if self.config.attributes_list().is_empty() {
            "name, type, description".to_string()
        } else {
            self.config.attributes_list().join(", ")
        };

        let extraction_prompt = EXTRACTION_PROMPT
            .replace("{entity_types}", &entity_types)
            .replace("{relationship_types}", &rel_types)
            .replace("{attribute_types}", &attr_types)
            .replace("{context}", &self.context)
            .replace("{chunk}", chunk);

        let opts = CompletionOptions {
            model: self.config.model_name.clone(),
            temperature: 0.3,
            max_tokens: 6500,
        };

        // Drive the whole chunk through ONE multi-turn session: native for
        // backends with a real conversation protocol (the SDK retains context
        // across turns), or a history-replaying fallback for the rest. Both
        // expose the same `send` contract, so the gleaning logic below is
        // transport-agnostic.
        let system = SYSTEM_PROMPT.to_string();
        let mut session: Box<dyn ChatSession> =
            match self.backend.open_session(Some(system.clone()), &opts).await {
                Ok(Some(s)) => s,
                Ok(None) => {
                    Box::new(ReplaySession::new(self.backend.clone(), Some(system), opts.clone()))
                }
                Err(e) => {
                    if !self.quiet {
                        eprintln!("Session open error: {e}");
                    }
                    return (Vec::new(), KnowledgeGraph::new());
                }
            };

        let mut all_entities: HashMap<String, Entity> = HashMap::new();
        let mut all_triples: Vec<Triple> = Vec::new();
        let mut parsed_results: Vec<ParsedResult> = Vec::new();

        // Entity gleaning: the extraction turn, then "what did you miss?" turns.
        for i in 0..=self.max_gleanings {
            let prompt: &str = if i == 0 { &extraction_prompt } else { CONTINUE_PROMPT };
            let output = match session.send(prompt).await {
                Ok(o) => o.trim().to_string(),
                Err(e) => {
                    if !self.quiet {
                        eprintln!("Extraction error: {e}");
                    }
                    break;
                }
            };
            if output.is_empty() {
                continue;
            }

            let parsed = parse_output(&output, &self.config);
            let new_entities = parsed.entities.len();
            let new_triples = parsed.triples.len();
            for (id, e) in &parsed.entities {
                all_entities.entry(id.clone()).or_insert_with(|| e.clone());
            }
            all_triples.extend(parsed.triples.clone());
            parsed_results.push(parsed);

            // Early stop on a gleaning round that added nothing.
            if i > 0 && new_entities == 0 && new_triples == 0 {
                break;
            }
        }

        // Relation gleaning: name the entities that ended up with no edge and ask
        // the model — in the SAME session, so the source text is still in context
        // — to connect them. Each round only adds edges between already-known
        // entities, so it shrinks the orphan set without inventing dangling nouns.
        let rescue_rel_types = if self.config.predicates_list().is_empty() {
            "related_to, part_of, uses, produces, has_property".to_string()
        } else {
            self.config.predicates_list().join(", ")
        };
        for _ in 0..self.max_relation_gleanings {
            let linked: HashSet<&str> = all_triples
                .iter()
                .flat_map(|t| [t.subject.id.as_str(), t.object.id.as_str()])
                .collect();
            let orphans: Vec<&Entity> =
                all_entities.values().filter(|e| !linked.contains(e.id.as_str())).collect();
            if orphans.is_empty() {
                break;
            }
            let orphan_list =
                orphans.iter().map(|e| format!("- {}", e.label)).collect::<Vec<_>>().join("\n");
            let prompt = RELATION_GLEANING_PROMPT
                .replace("{orphans}", &orphan_list)
                .replace("{relationship_types}", &rescue_rel_types);

            let output = match session.send(&prompt).await {
                Ok(o) => o.trim().to_string(),
                Err(e) => {
                    if !self.quiet {
                        eprintln!("Relation-gleaning error: {e}");
                    }
                    break;
                }
            };
            if output.is_empty() || output.eq_ignore_ascii_case("no") {
                break;
            }

            // Keep only edges that resolve to known entities and are genuinely new.
            let mut seen: HashSet<(String, String, String)> =
                all_triples.iter().map(|t| t.to_tuple()).collect();
            let rescued: Vec<Triple> = parse_relations_against(&output, &all_entities)
                .into_iter()
                .filter(|t| seen.insert(t.to_tuple()))
                .collect();
            if rescued.is_empty() {
                break;
            }
            all_triples.extend(rescued);
        }

        if let Err(e) = session.finish().await {
            if !self.quiet {
                eprintln!("Session finish error: {e}");
            }
        }

        let mut kg = KnowledgeGraph::new();
        for e in all_entities.into_values() {
            kg.add_entity(e);
        }
        for t in all_triples {
            kg.add_triple(t);
        }
        (parsed_results, kg)
    }
}

/// Parse a relation-only LLM response into triples, resolving each endpoint
/// against an existing entity table (by normalised label). Unlike [`parse_output`],
/// it does not require the entities to be re-declared in this response — the
/// rescue round emits relationships only. Edges with an unknown endpoint are
/// dropped (no dangling/hallucinated nodes).
pub(crate) fn parse_relations_against(output: &str, known: &HashMap<String, Entity>) -> Vec<Triple> {
    let id_map: HashMap<String, String> =
        known.values().map(|e| (normalize_key(&e.label), e.id.clone())).collect();

    let mut triples = Vec::new();
    for item in split_to_items(output) {
        let mut item = item.trim().to_string();
        if item.is_empty() {
            continue;
        }
        if item.starts_with('(') && !item.ends_with(')') {
            item.push(')');
        }
        let head = item
            .trim_start_matches('(')
            .split(TUPLE_DELIMITER)
            .next()
            .unwrap_or("")
            .to_lowercase();
        if !(head.contains("relationship") && item.contains(TUPLE_DELIMITER)) {
            continue;
        }
        let Some(rel) = parse_relationship_item(&item, &id_map) else {
            continue;
        };
        // Both endpoints must be entities we already know about.
        let (Some(s), Some(o)) = (known.get(&rel.source_id), known.get(&rel.target_id)) else {
            continue;
        };
        // Mirror `parse_output`'s triple construction exactly (incl. the
        // field-shift quirk: `rel.description` carries the relationship-type token).
        let predicate_type = infer_predicate_type(&rel.description);
        let label: String = rel.description.chars().take(50).collect();
        let predicate = Predicate::with_label(predicate_type, label);
        let mut t = Triple::new(s.clone(), predicate, o.clone());
        t.confidence = Some(rel.strength);
        t.metadata.insert("description".into(), serde_json::json!(rel.description));
        t.metadata.insert("source_name".into(), serde_json::json!(rel.source_name));
        t.metadata.insert("target_name".into(), serde_json::json!(rel.target_name));
        t.metadata.insert("relation_gleaned".into(), serde_json::json!(true));
        triples.push(t);
    }
    triples
}

#[async_trait::async_trait]
impl Extractor for SimpleExtractor {
    async fn extract(&self, text: &str) -> anyhow::Result<ExtractionResponse> {
        validate_input(text, self.config.min_segment_size, self.quiet)?;

        // Segment only when the text exceeds segment_size; otherwise a single
        // chunk preserves the original single-shot + gleaning behaviour exactly.
        let chunks: Vec<String> = if text.chars().count() > self.config.segment_size {
            segment(text, self.config.chunker, self.config.segment_size, self.config.overlap)
                .into_iter()
                .enumerate()
                .filter(|(i, s)| !(s.content.chars().count() < self.config.min_segment_size && *i > 0))
                .map(|(_, s)| s.content)
                .collect()
        } else {
            vec![text.to_string()]
        };

        // Extract chunks concurrently. LLM calls are I/O-bound, so `buffered`
        // runs up to `max_concurrency` in flight while preserving chunk order.
        let max_conc = self.config.max_concurrency.max(1);
        let per_chunk: Vec<(Vec<ParsedResult>, KnowledgeGraph)> = stream::iter(chunks)
            .map(|chunk| async move { self.extract_chunk(&chunk).await })
            .buffered(max_conc)
            .collect()
            .await;

        let mut parsed_results: Vec<ParsedResult> = Vec::new();
        let mut graphs: Vec<KnowledgeGraph> = Vec::new();
        for (prs, kg) in per_chunk {
            parsed_results.extend(prs);
            graphs.push(kg);
        }

        // Fold the per-chunk graphs, deduplicating per the configured strategy.
        let kg = if self.config.spec.merge_duplicates {
            let strategy = self.config.spec.merge_strategy;
            if strategy.needs_backend() {
                let opts = CompletionOptions {
                    model: self.config.model_name.clone(),
                    temperature: 0.3,
                    max_tokens: 1000,
                };
                merge_all_dedup_llm(graphs, &self.backend, &opts).await
            } else {
                graphs.into_iter().fold(KnowledgeGraph::new(), |acc, g| {
                    merge_knowledge_graphs_with(acc, g, true, strategy)
                })
            }
        } else {
            merge_all(graphs)
        };

        let mut resp = ExtractionResponse::new(kg);
        resp.parsed_results = parsed_results;
        resp.config = Some(self.config.clone());
        Ok(resp)
    }
}

/// Parse delimiter-formatted LLM output into entities + triples.
pub(crate) fn parse_output(output: &str, _config: &ExtractionConfig) -> ParsedResult {
    let items = split_to_items(output);
    let mut entities: HashMap<String, Entity> = HashMap::new();
    let mut entity_id_map: HashMap<String, String> = HashMap::new();
    let mut relationships: Vec<RelData> = Vec::new();

    for item in &items {
        let mut item = item.trim().to_string();
        if item.is_empty() {
            continue;
        }
        if item.starts_with('(') && !item.ends_with(')') {
            item.push(')');
        }
        // Classify by the record's leading token, not a substring scan of the
        // whole line: a relationship whose description mentions "entity" (e.g.
        // "these two entities are related") must not be misrouted to the entity
        // branch and silently dropped.
        let head = item
            .trim_start_matches('(')
            .split(TUPLE_DELIMITER)
            .next()
            .unwrap_or("")
            .to_lowercase();
        if head.contains("entity") && item.contains(TUPLE_DELIMITER) {
            if let Some(e) = parse_entity_item(&item) {
                entity_id_map.insert(normalize_key(&e.label), e.id.clone());
                entities.insert(e.id.clone(), e);
            }
        } else if head.contains("relationship") && item.contains(TUPLE_DELIMITER) {
            if let Some(r) = parse_relationship_item(&item, &entity_id_map) {
                relationships.push(r);
            }
        }
    }

    let mut triples = Vec::new();
    for rel in &relationships {
        let (Some(s), Some(o)) = (entities.get(&rel.source_id), entities.get(&rel.target_id)) else {
            continue;
        };
        let predicate_type = infer_predicate_type(&rel.description);
        let label: String = rel.description.chars().take(50).collect();
        let predicate = Predicate::with_label(predicate_type, label);
        let mut t = Triple::new(s.clone(), predicate, o.clone());
        t.confidence = Some(rel.strength);
        t.metadata.insert("description".into(), serde_json::json!(rel.description));
        t.metadata.insert("source_name".into(), serde_json::json!(rel.source_name));
        t.metadata.insert("target_name".into(), serde_json::json!(rel.target_name));
        triples.push(t);
    }

    let mut metadata = HashMap::new();
    metadata.insert("item_count".into(), serde_json::json!(items.len()));
    ParsedResult {
        raw_response: output.to_string(),
        entities_and_triples: Vec::new(),
        entities,
        relationships: Vec::new(),
        triples,
        metadata,
    }
}

/// Recover each entity record's **raw type token** (normalized name → raw type
/// string), which [`parse_output`] discards by mapping it into the
/// [`EntityType`] enum. Fixed-schema validation needs the literal token so a
/// domain-specific schema type (one outside the known `EntityType` vocabulary,
/// which `from_loose` would collapse to `Other`) can still be matched. Mirrors
/// [`parse_entity_item`]'s field positions exactly; keyed by [`normalize_key`]
/// so it lines up with an [`Entity`]'s lowercased label.
pub(crate) fn entity_type_tokens(output: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for item in split_to_items(output) {
        let mut item = item.trim().to_string();
        if item.is_empty() {
            continue;
        }
        if item.starts_with('(') && !item.ends_with(')') {
            item.push(')');
        }
        let inner = strip_parens(&item);
        let parts: Vec<&str> = inner.split(TUPLE_DELIMITER).collect();
        if parts.len() < 3 || !parts[0].to_lowercase().contains("entity") {
            continue;
        }
        let name = clean_entity_name(parts[1]);
        if name.is_empty() {
            continue;
        }
        let type_token = parts[2].trim().trim_matches(|c| "<> \"*#".contains(c)).to_string();
        out.insert(normalize_key(&name), type_token);
    }
    out
}

struct RelData {
    source_id: String,
    target_id: String,
    source_name: String,
    target_name: String,
    description: String,
    strength: f64,
}

fn split_to_items(results: &str) -> Vec<String> {
    let entity_re = Regex::new(r#"\(\s*"?entity"?\s*<\|>"#).unwrap();
    let rel_re = Regex::new(r#"\(\s*"?relationship"?\s*<\|>"#).unwrap();
    let mut items = Vec::new();
    for part in results.split(RECORD_DELIMITER) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut starts: Vec<usize> = entity_re
            .find_iter(part)
            .chain(rel_re.find_iter(part))
            .map(|m| m.start())
            .collect();
        starts.sort_unstable();
        if starts.len() <= 1 {
            items.push(part.to_string());
        } else {
            for i in 0..starts.len() {
                let s = starts[i];
                let slice = if i == starts.len() - 1 {
                    &part[s..]
                } else {
                    &part[s..starts[i + 1]]
                };
                items.push(slice.trim().to_string());
            }
        }
    }
    items
}

fn parse_entity_item(item: &str) -> Option<Entity> {
    let inner = strip_parens(item);
    let parts: Vec<&str> = inner.split(TUPLE_DELIMITER).collect();
    if parts.len() < 3 {
        return None;
    }
    let head = parts[0];
    if !head.to_lowercase().contains("entity") {
        return None;
    }
    let components = &parts[1..];
    let name = components.first().copied().unwrap_or("");
    let type_str = components.get(1).copied().unwrap_or("");
    let description = components.get(2).copied().unwrap_or("");
    let attrs = components.get(3).copied().unwrap_or("");

    let name = clean_entity_name(name);
    if name.is_empty() {
        return None;
    }
    let type_clean = type_str.trim().trim_matches(|c| "<> \"*#".contains(c)).to_lowercase();
    let description = clean_description(description);
    let metadata = parse_attributes_string(attrs);

    let mut entity = Entity::new(entity_id(&name), &name, map_to_entity_type(&type_clean));
    entity.description = Some(description);
    if !metadata.is_empty() {
        entity.metadata = metadata;
    }
    Some(entity)
}

fn parse_relationship_item(item: &str, id_map: &HashMap<String, String>) -> Option<RelData> {
    let inner = strip_parens(item);
    let parts: Vec<&str> = inner.split(TUPLE_DELIMITER).collect();
    if parts.len() < 4 {
        return None;
    }
    if !parts[0].to_lowercase().contains("relationship") {
        return None;
    }
    // NOTE: faithful to the Python `source, target, description, *strength = components`
    // destructuring. The emitted tuple is
    // (relationship, source, target, REL_TYPE, description, strength), so the
    // *relationship-type* token lands in `description` (and is what drives
    // predicate inference), while the real description text falls into the
    // first strength element. Preserved deliberately for parity.
    let components = &parts[1..];
    let source = clean_entity_name(components.first().copied().unwrap_or(""));
    let target = clean_entity_name(components.get(1).copied().unwrap_or(""));
    let description = clean_description(components.get(2).copied().unwrap_or(""));

    let strength = components
        .get(3)
        .map(|s| {
            let digits: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
            digits.parse::<f64>().map(|f| f.clamp(0.0, 1.0)).unwrap_or(0.8)
        })
        .unwrap_or(0.8);

    Some(RelData {
        source_id: resolve_entity_id(&source, id_map),
        target_id: resolve_entity_id(&target, id_map),
        source_name: source,
        target_name: target,
        description,
        strength,
    })
}

fn strip_parens(item: &str) -> String {
    let item = item.trim();
    if item.starts_with('(') {
        if let Some(close) = item.rfind(')') {
            if close > 0 {
                return item[1..close].to_string();
            }
        }
    }
    item.to_string()
}

fn normalize_key(name: &str) -> String {
    name.to_lowercase().trim().to_string()
}

fn resolve_entity_id(name: &str, id_map: &HashMap<String, String>) -> String {
    let key = normalize_key(name);
    id_map.get(&key).cloned().unwrap_or_else(|| entity_id(name))
}

fn clean_entity_name(name: &str) -> String {
    let name = name.trim_matches(|c| "<> \"'".contains(c));
    let no_quotes = Regex::new(r#"["'`]"#).unwrap().replace_all(name, "");
    let collapsed = Regex::new(r"\s+").unwrap().replace_all(no_quotes.trim(), " ");
    title_case(collapsed.trim())
}

fn clean_description(desc: &str) -> String {
    if desc.is_empty() {
        return "No description available".to_string();
    }
    let d = desc.trim_matches(|c| "<> \"'".contains(c));
    let d = Regex::new(r#"["'<>]"#).unwrap().replace_all(d, "");
    let d = Regex::new(r"\)\*\*|\*\*|\*\*##|\)").unwrap().replace_all(d.trim(), "");
    let d = Regex::new(r"\s+").unwrap().replace_all(d.trim(), " ");
    let out = d.trim().to_string();
    if out.is_empty() {
        "No description available".to_string()
    } else {
        out
    }
}

fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars.flat_map(|c| c.to_lowercase())).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn map_to_entity_type(type_str: &str) -> EntityType {
    EntityType::from_loose(type_str)
}

fn infer_predicate_type(description: &str) -> PredicateType {
    let d = description.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| d.contains(w));
    if has(&["use", "utilize", "employ", "apply"]) {
        PredicateType::Uses
    } else if has(&["part of", "belong", "component", "member"]) {
        PredicateType::PartOf
    } else if has(&["located", "based in", "situated"]) {
        PredicateType::LocatedIn
    } else if has(&["work", "employed", "affiliated"]) {
        PredicateType::WorksFor
    } else if has(&["own", "possess", "has"]) {
        PredicateType::HasProperty
    } else {
        PredicateType::RelatedTo
    }
}

/// Parse `"key: value, key2: value2"` (or a JSON object) into metadata.
fn parse_attributes_string(s: &str) -> HashMap<String, serde_json::Value> {
    let s = s.trim();
    let mut out = HashMap::new();
    if s.is_empty() {
        return out;
    }
    if s.starts_with('{') && s.ends_with('}') {
        if let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(s) {
            return m.into_iter().collect();
        }
    }
    // Split on top-level commas.
    let mut pairs = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    for ch in s.chars().chain(std::iter::once(',')) {
        match ch {
            '(' | '{' | '[' => {
                depth += 1;
                current.push(ch);
            }
            ')' | '}' | ']' => {
                // Clamp at 0: an unmatched closing bracket must not drive depth
                // negative, which would suppress the depth==0 comma split (and the
                // trailing sentinel flush) and silently drop every attribute.
                depth = (depth - 1).max(0);
                current.push(ch);
            }
            ',' if depth == 0 => {
                let t = current.trim().to_string();
                if !t.is_empty() {
                    pairs.push(t);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    for pair in pairs {
        if let Some((k, v)) = pair.split_once(':') {
            let key = k.trim().trim_matches(|c| "\"'".contains(c)).to_string();
            let val = v.trim().trim_matches(|c| "\"'".contains(c));
            let value = if val.eq_ignore_ascii_case("true") {
                serde_json::json!(true)
            } else if val.eq_ignore_ascii_case("false") {
                serde_json::json!(false)
            } else if let Ok(i) = val.parse::<i64>() {
                serde_json::json!(i)
            } else if let Ok(f) = val.parse::<f64>() {
                serde_json::json!(f)
            } else {
                serde_json::json!(val)
            };
            out.insert(key, value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MockBackend;

    #[tokio::test]
    async fn extracts_entities_and_relationship() {
        // 4th tuple field is the relationship type; it drives predicate inference
        // (the field-shift quirk documented in parse_relationship_item).
        let resp_text = "(entity<|>OpenAI<|>organization<|>An AI research lab.<|>)##\
            (entity<|>GPT-4<|>technology<|>A large language model.<|>)##\
            (relationship<|>OpenAI<|>GPT-4<|>uses<|>OpenAI develops GPT-4.<|>0.9)##";
        // Second gleaning returns nothing new → early stop.
        let backend = Arc::new(MockBackend::new(vec![resp_text.into(), String::new()]));
        let ex = SimpleExtractor::new(backend);
        let out = ex.extract("OpenAI developed GPT-4.").await.unwrap();
        assert_eq!(out.num_entities(), 2);
        assert_eq!(out.num_triples(), 1);
        assert_eq!(out.knowledge_graph.triples[0].predicate.predicate_type, PredicateType::Uses);
    }

    #[tokio::test]
    async fn relation_gleaning_rescues_orphan_entities() {
        // First call extracts two entities but NO relationship -> both orphan.
        let extract = "(entity<|>OpenAI<|>organization<|>An AI research lab.<|>)##\
            (entity<|>GPT-4<|>technology<|>A large language model.<|>)##";
        // The rescue round emits a relationship only (no entity records). It must
        // still resolve against the already-known entities and become a triple.
        let rescue = "(relationship<|>OpenAI<|>GPT-4<|>uses<|>OpenAI develops GPT-4.<|>0.9)##";
        let backend = Arc::new(MockBackend::new(vec![extract.into(), rescue.into()]));
        let mut ex = SimpleExtractor::new(backend);
        ex.max_gleanings = 0; // exactly one extraction call, then one rescue round
        ex.max_relation_gleanings = 1;

        let out = ex.extract("OpenAI developed GPT-4.").await.unwrap();
        assert_eq!(out.num_entities(), 2);
        assert_eq!(out.num_triples(), 1, "the orphan rescue round should add the edge");
        let t = &out.knowledge_graph.triples[0];
        assert_eq!(t.predicate.predicate_type, PredicateType::Uses);
        assert_eq!(t.metadata.get("relation_gleaned"), Some(&serde_json::json!(true)));
    }

    #[tokio::test]
    async fn relation_gleaning_off_by_default_keeps_orphans() {
        let extract = "(entity<|>OpenAI<|>organization<|>An AI research lab.<|>)##\
            (entity<|>GPT-4<|>technology<|>A large language model.<|>)##";
        let backend = Arc::new(MockBackend::new(vec![extract.into(), String::new()]));
        let mut ex = SimpleExtractor::new(backend);
        ex.max_gleanings = 0;
        // max_relation_gleanings defaults to 0 -> no rescue, orphans stay orphan.
        let out = ex.extract("OpenAI developed GPT-4.").await.unwrap();
        assert_eq!(out.num_entities(), 2);
        assert_eq!(out.num_triples(), 0);
    }

    #[tokio::test]
    async fn relationship_with_entity_in_text_is_not_dropped() {
        // The relationship description mentions "entities"; the record must be
        // classified by its leading token, not a whole-line substring scan.
        let resp = "(entity<|>OpenAI<|>organization<|>An AI lab.<|>)##\
            (entity<|>GPT-4<|>technology<|>A model.<|>)##\
            (relationship<|>OpenAI<|>GPT-4<|>uses<|>OpenAI is the parent entity of GPT-4.<|>0.9)##";
        let backend = Arc::new(MockBackend::new(vec![resp.into(), String::new()]));
        let out = SimpleExtractor::new(backend).extract("OpenAI and GPT-4.").await.unwrap();
        assert_eq!(out.num_entities(), 2);
        assert_eq!(
            out.num_triples(),
            1,
            "relationship must survive even though its text contains the word 'entity'"
        );
    }

    #[test]
    fn attributes_unmatched_bracket_does_not_drop_all() {
        // A stray `]` previously drove the depth counter negative, suppressing
        // every comma split so the whole attribute set was lost.
        let attrs = parse_attributes_string("role: lead], team: platform");
        assert!(attrs.contains_key("role"), "first attribute must survive: {attrs:?}");
        assert!(attrs.contains_key("team"), "later attribute must survive: {attrs:?}");
    }

    #[test]
    fn entity_id_is_deterministic_md5() {
        assert_eq!(entity_id("Openai"), entity_id("Openai"));
        assert!(entity_id("X").starts_with("entity_"));
    }

    #[tokio::test]
    async fn segments_long_text_and_merges_chunk_graphs() {
        use crate::types::ChunkStrategy;
        // Two Char chunks, one LLM call each (gleaning off). Each chunk's canned
        // response carries a distinct entity; the merged graph must hold both —
        // proving segment → concurrent per-chunk extract → merge end to end.
        let r0 = "(entity<|>Alpha<|>organization<|>First chunk entity.<|>)##";
        let r1 = "(entity<|>Beta<|>organization<|>Second chunk entity.<|>)##";
        let backend = Arc::new(MockBackend::new(vec![r0.into(), r1.into()]));

        let mut cfg = SimpleExtractor::default_config();
        cfg.segment_size = 20;
        cfg.overlap = 0;
        cfg.min_segment_size = 1;
        cfg.chunker = ChunkStrategy::Char;
        cfg.max_concurrency = 2;
        let mut ex = SimpleExtractor::with_config(backend, cfg);
        ex.max_gleanings = 0; // one call per chunk → deterministic response mapping

        // 40 chars → exactly two 20-char Char chunks.
        let text = "a".repeat(20) + &"b".repeat(20);
        let out = ex.extract(&text).await.unwrap();

        let labels: Vec<&str> =
            out.knowledge_graph.entities.values().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"Alpha"), "chunk 0 entity must be present: {labels:?}");
        assert!(labels.contains(&"Beta"), "chunk 1 entity must be present: {labels:?}");
        assert_eq!(out.num_entities(), 2);
    }

    #[test]
    fn entity_type_tokens_recovers_raw_types() {
        // Recovers the literal type token (keyed by lowercased name), including a
        // custom one the EntityType enum would collapse to `Other`. Relationship
        // records are ignored.
        let output = "(entity<|>OpenAI<|>organization<|>An AI lab.<|>)##\
            (entity<|>Widget X<|>GADGET<|>A custom thing.<|>)##\
            (relationship<|>OpenAI<|>Widget X<|>builds<|>they build it<|>0.9)";
        let tokens = entity_type_tokens(output);
        assert_eq!(tokens.get("openai").map(String::as_str), Some("organization"));
        assert_eq!(tokens.get("widget x").map(String::as_str), Some("GADGET"));
        assert_eq!(tokens.len(), 2, "relationship record must not appear");
    }
}
