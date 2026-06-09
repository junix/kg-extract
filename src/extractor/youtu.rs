//! YoutuExtractor — schema-driven extraction with optional agent mode (schema
//! evolution) and community detection. Ported from `graph/kg_extractor/youtu.py`.

use super::{validate_input, Extractor};
use crate::backend::{CompletionOptions, LlmBackend};
use crate::parser::extract_json_from_response;
use crate::types::{
    Entity, EntityType, ExtractionConfig, ExtractionResponse, KnowledgeGraph, Predicate,
    PredicateType, Schema, Triple,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Extraction mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YoutuMode {
    /// Fixed schema: use only the schema's types.
    NoAgent,
    /// Allow the model to propose new schema types (`new_schema_types`).
    Agent,
}

impl YoutuMode {
    fn as_str(&self) -> &'static str {
        match self {
            YoutuMode::NoAgent => "noagent",
            YoutuMode::Agent => "agent",
        }
    }
}

/// Schema-based knowledge graph extractor.
pub struct YoutuExtractor {
    backend: Arc<dyn LlmBackend>,
    config: ExtractionConfig,
    pub mode: YoutuMode,
    pub enable_community_detection: bool,
    pub quiet: bool,
}

impl YoutuExtractor {
    pub fn default_config() -> ExtractionConfig {
        // Youtu base config starts from an EMPTY schema (no default seeding).
        ExtractionConfig {
            extraction_schema: Schema::default(),
            model_name: "qwen-max".into(),
            segment_size: 3000,
            min_segment_size: 100,
            ..Default::default()
        }
    }

    pub fn new(backend: Arc<dyn LlmBackend>) -> Self {
        YoutuExtractor {
            backend,
            config: Self::default_config(),
            mode: YoutuMode::NoAgent,
            enable_community_detection: false,
            quiet: false,
        }
    }

    pub fn with_config(backend: Arc<dyn LlmBackend>, config: ExtractionConfig) -> Self {
        YoutuExtractor {
            backend,
            config,
            mode: YoutuMode::NoAgent,
            enable_community_detection: false,
            quiet: false,
        }
    }

    pub fn mode(mut self, mode: YoutuMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn community_detection(mut self, enable: bool) -> Self {
        self.enable_community_detection = enable;
        self
    }

    pub fn config(&self) -> &ExtractionConfig {
        &self.config
    }

    fn build_prompt(&self, text: &str) -> String {
        let schema = serde_json::json!({
            "nodes": self.config.entity_types_list(),
            "relations": self.config.predicates_list(),
            "attributes": self.config.attributes_list(),
        });
        let schema_json = serde_json::to_string(&schema).unwrap_or_default();

        match self.mode {
            YoutuMode::Agent => format!(
                "Extract entities and relationships from the following text using the provided schema as guidance.\n\
You may suggest new entity types, relations, or attributes if they better represent the content.\n\n\
Schema:\n{schema_json}\n\n\
Text:\n{text}\n\n\
Output JSON with:\n\
1. \"entities\": {{\"entity_name\": {{\"type\": \"EntityType\", \"attributes\": {{\"attr\": \"value\"}}}}}}\n\
2. \"relationships\": [[\"subject\", \"predicate\", \"object\"]]\n\
3. \"entity_types\": {{\"entity_name\": \"type\"}} (map entities to their types)\n\
4. \"new_schema_types\": {{\"nodes\": [], \"relations\": [], \"attributes\": []}} (if suggesting new types)\n\n\
Ensure valid JSON output."
            ),
            YoutuMode::NoAgent => format!(
                "Extract entities and relationships from the following text using the provided schema.\n\n\
Schema:\n{schema_json}\n\n\
Text:\n{text}\n\n\
Output JSON with:\n\
1. \"entities\": {{\"entity_name\": {{\"type\": \"EntityType\", \"attributes\": {{\"attr\": \"value\"}}}}}}\n\
2. \"relationships\": [[\"subject\", \"predicate\", \"object\"]]\n\
3. \"entity_types\": {{\"entity_name\": \"type\"}} (map entities to their types)\n\n\
Use only the entity types and relations from the schema.\n\
Ensure valid JSON output."
            ),
        }
    }

    fn build_graph(&self, data: &serde_json::Value) -> KnowledgeGraph {
        let mut entities: HashMap<String, Entity> = HashMap::new();
        let entity_data = data.get("entities").and_then(|v| v.as_object());
        let entity_types = data.get("entity_types").and_then(|v| v.as_object());

        // Preserve insertion order while building name→entity for relationships.
        let mut name_to_id: HashMap<String, String> = HashMap::new();
        let mut kg = KnowledgeGraph::new();

        if let Some(obj) = entity_data {
            for (name, info) in obj {
                let id = entity_id(name);
                let (type_str, attributes) = if let Some(io) = info.as_object() {
                    let t = io
                        .get("type")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .or_else(|| {
                            entity_types
                                .and_then(|et| et.get(name))
                                .and_then(|v| v.as_str())
                                .map(String::from)
                        })
                        .unwrap_or_else(|| "UNKNOWN".into());
                    let attrs: HashMap<String, serde_json::Value> = io
                        .get("attributes")
                        .and_then(|v| v.as_object())
                        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default();
                    (t, attrs)
                } else {
                    let t = entity_types
                        .and_then(|et| et.get(name))
                        .and_then(|v| v.as_str())
                        .unwrap_or("UNKNOWN")
                        .to_string();
                    (t, HashMap::new())
                };

                // Youtu uses strict name match, fallback PHYSICAL_OBJECT.
                let entity_type = type_str
                    .to_uppercase()
                    .replace([' ', '-'], "_")
                    .parse::<EntityType>()
                    .unwrap_or(EntityType::PhysicalObject);

                let mut entity = Entity::new(id.clone(), name.clone(), entity_type);
                entity.description = attributes
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                entity.metadata = attributes;
                name_to_id.insert(name.clone(), id.clone());
                entities.insert(id.clone(), entity.clone());
                kg.add_entity(entity);
            }
        }

        if let Some(rels) = data.get("relationships").and_then(|v| v.as_array()) {
            for rel in rels {
                let Some(arr) = rel.as_array() else { continue };
                if arr.len() < 3 {
                    continue;
                }
                let subject_name = arr[0].as_str().unwrap_or_default();
                let predicate_str = arr[1].as_str().unwrap_or_default();
                let object_name = arr[2].as_str().unwrap_or_default();

                let (Some(sid), Some(oid)) =
                    (name_to_id.get(subject_name), name_to_id.get(object_name))
                else {
                    continue;
                };
                let predicate_type = predicate_str
                    .to_uppercase()
                    .replace([' ', '-'], "_")
                    .parse::<PredicateType>()
                    .unwrap_or(PredicateType::RelatedTo);
                let predicate = Predicate::with_label(predicate_type, predicate_str);
                let triple = Triple::new(
                    entities[sid].clone(),
                    predicate,
                    entities[oid].clone(),
                );
                kg.add_triple(triple);
            }
        }

        kg
    }
}

#[async_trait::async_trait]
impl Extractor for YoutuExtractor {
    async fn extract(&self, text: &str) -> anyhow::Result<ExtractionResponse> {
        validate_input(text, self.config.min_segment_size, self.quiet)?;

        let prompt = self.build_prompt(text);
        let opts = CompletionOptions {
            model: self.config.model_name.clone(),
            temperature: 0.3,
            max_tokens: 4000,
        };
        let response = match self.backend.complete_prompt(&prompt, &opts).await {
            Ok(r) => r,
            Err(e) => {
                if !self.quiet {
                    eprintln!("Error calling LLM: {e}");
                }
                return Ok(ExtractionResponse::new(KnowledgeGraph::new()));
            }
        };

        let data = extract_json_from_response(&response)
            .unwrap_or_else(|| serde_json::json!({"entities": {}, "relationships": []}));

        let mut kg = self.build_graph(&data);
        if self.enable_community_detection {
            apply_community_detection(&mut kg);
        }

        let mut resp = ExtractionResponse::new(kg);
        resp.metadata.insert("model".into(), serde_json::json!(self.config.model_name));
        resp.metadata.insert("mode".into(), serde_json::json!(self.mode.as_str()));
        resp.metadata.insert(
            "schema_used".into(),
            serde_json::json!({
                "entity_types": self.config.entity_types_list(),
                "predicates": self.config.predicates_list(),
                "attributes": self.config.attributes_list(),
            }),
        );
        if let Some(new_schema) = data.get("new_schema_types") {
            resp.metadata.insert("new_schema_types".into(), new_schema.clone());
        }
        resp.config = Some(self.config.clone());
        Ok(resp)
    }
}

/// Stable id for an entity name, matching simple.rs / toolcall.rs / mcp.rs so
/// Youtu output is deterministic and interoperable (id == `entity_<md5(name)[..8]>`).
fn entity_id(name: &str) -> String {
    let digest = format!("{:x}", md5::compute(name.as_bytes()));
    format!("entity_{}", &digest[..8])
}

/// Assign `community_id` to each entity via label propagation on the undirected
/// projection of the graph (dependency-free analogue of networkx greedy
/// modularity communities).
fn apply_community_detection(kg: &mut KnowledgeGraph) {
    let ids: Vec<String> = kg.entities.keys().cloned().collect();
    if ids.is_empty() {
        return;
    }
    let index: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();

    let mut adj: Vec<HashSet<usize>> = vec![HashSet::new(); ids.len()];
    for t in &kg.triples {
        if let (Some(&a), Some(&b)) = (index.get(t.subject.id.as_str()), index.get(t.object.id.as_str())) {
            if a != b {
                adj[a].insert(b);
                adj[b].insert(a);
            }
        }
    }

    // Label propagation: each node starts in its own community; iterate to the
    // most common neighbour label. Deterministic tie-break by lowest label.
    let mut labels: Vec<usize> = (0..ids.len()).collect();
    for _ in 0..10 {
        let mut changed = false;
        for v in 0..ids.len() {
            if adj[v].is_empty() {
                continue;
            }
            let mut counts: HashMap<usize, usize> = HashMap::new();
            for &n in &adj[v] {
                *counts.entry(labels[n]).or_insert(0) += 1;
            }
            if let Some((&best, _)) =
                counts.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            {
                if labels[v] != best {
                    labels[v] = best;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Renumber communities to 0..k in first-seen order.
    let mut remap: HashMap<usize, usize> = HashMap::new();
    for (i, id) in ids.iter().enumerate() {
        let next = remap.len();
        let comm = *remap.entry(labels[i]).or_insert(next);
        if let Some(e) = kg.entities.get_mut(id) {
            e.metadata.insert("community_id".into(), serde_json::json!(comm));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MockBackend;

    #[tokio::test]
    async fn schema_based_extraction_with_community() {
        let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"},
                                      "GPT-4": {"type": "TECHNOLOGY"}},
                        "relationships": [["OpenAI", "DEVELOPED_BY", "GPT-4"]]}"#;
        let backend = Arc::new(MockBackend::single(json));
        let ex = YoutuExtractor::new(backend).community_detection(true);
        let out = ex.extract("OpenAI developed GPT-4.").await.unwrap();
        assert_eq!(out.num_entities(), 2);
        assert_eq!(out.num_triples(), 1);
        for e in out.knowledge_graph.entities.values() {
            assert!(e.metadata.contains_key("community_id"));
        }
    }

    #[tokio::test]
    async fn entity_ids_are_deterministic_md5() {
        let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"}}, "relationships": []}"#;
        let a = YoutuExtractor::new(Arc::new(MockBackend::single(json)))
            .extract("text")
            .await
            .unwrap();
        let b = YoutuExtractor::new(Arc::new(MockBackend::single(json)))
            .extract("text")
            .await
            .unwrap();
        let ka: Vec<&String> = a.knowledge_graph.entities.keys().collect();
        let kb: Vec<&String> = b.knowledge_graph.entities.keys().collect();
        assert_eq!(ka, kb, "Youtu entity ids must be deterministic across runs");
        let expected = entity_id("OpenAI");
        assert!(
            a.knowledge_graph.entities.contains_key(&expected),
            "id must follow the shared md5(name) scheme"
        );
    }

    #[tokio::test]
    async fn agent_mode_captures_new_schema() {
        let json = r#"{"entities": {"Movie X": {"type": "WORK_OF_ART"}},
                        "relationships": [],
                        "new_schema_types": {"nodes": ["Movie"], "relations": ["starring"], "attributes": []}}"#;
        let backend = Arc::new(MockBackend::single(json));
        let ex = YoutuExtractor::new(backend).mode(YoutuMode::Agent);
        let out = ex.extract("Some text about a movie.").await.unwrap();
        assert!(out.metadata.contains_key("new_schema_types"));
        assert_eq!(out.metadata["mode"], serde_json::json!("agent"));
    }
}
