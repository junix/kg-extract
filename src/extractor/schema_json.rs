//! SchemaJsonExtractor — schema-driven extraction with three schema modes:
//! `Open` (no predefined types), `Fixed` (closed schema), and `Evolving`
//! (seed schema the model may extend). Ported from `graph/kg_extractor/youtu.py`.

use super::{validate_input, Extractor, SchemaMode};
use crate::backend::{CompletionOptions, LlmBackend, Message};
use crate::graph_build::GraphBuilder;
use crate::parser::extract_json_from_response;
use crate::types::{
    EntityType, ExtractionConfig, ExtractionResponse, ExtractionSpec, KnowledgeGraph,
    MergeStrategy, Predicate, PredicateType, Schema,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

/// Direction rule shared by every schema-json system prompt: triples must read
/// left-to-right as a true sentence, so passive `*_BY` predicates are not
/// emitted reversed ("Dario Amodei FOUNDED_BY Anthropic").
const DIRECTION_RULE: &str = "Direction rule: each [\"subject\", \"predicate\", \"object\"] triple must read left-to-right as a TRUE sentence. \
EVERY predicate ending in _BY (FOUNDED_BY, DEVELOPED_BY, CREATED_BY, INVENTED_BY, PUBLISHED_BY, ...) is passive: the subject is the thing acted on, the object is the doer \
([\"Anthropic\", \"FOUNDED_BY\", \"Dario Amodei\"], never [\"Dario Amodei\", \"FOUNDED_BY\", \"Anthropic\"]); a person is never the subject of a *_BY triple about their own work. \
Active predicates (FOUNDED, DEVELOPED, USES) point from the doer to the thing. \
If the sentence is false as written, swap subject and object or pick the opposite-voice predicate.";

/// Schema-based knowledge graph extractor.
pub struct SchemaJsonExtractor {
    backend: Arc<dyn LlmBackend>,
    config: ExtractionConfig,
    pub quiet: bool,
}

/// Normalize a type token for schema matching: trimmed, uppercased,
/// spaces/dashes → underscores (same canonical form `build_graph` parses into).
fn norm_type(s: &str) -> String {
    s.trim().to_uppercase().replace([' ', '-'], "_")
}

/// What Fixed-mode enforcement removed from a response.
#[derive(Default)]
struct FixedDrops {
    /// Total records dropped (entities + relations).
    records: usize,
    /// Distinct out-of-schema type names that triggered a drop.
    types: BTreeSet<String>,
}

impl SchemaJsonExtractor {
    pub fn default_config() -> ExtractionConfig {
        // SchemaJson base config starts from an EMPTY schema (no default seeding).
        ExtractionConfig {
            spec: ExtractionSpec {
                schema: Schema::default(),
                ..Default::default()
            },
            model_name: "qwen-max".into(),
            segment_size: 3000,
            min_segment_size: 100,
            ..Default::default()
        }
    }

    pub fn new(backend: Arc<dyn LlmBackend>) -> Self {
        SchemaJsonExtractor {
            backend,
            config: Self::default_config(),
            quiet: false,
        }
    }

    pub fn with_config(backend: Arc<dyn LlmBackend>, config: ExtractionConfig) -> Self {
        SchemaJsonExtractor {
            backend,
            config,
            quiet: false,
        }
    }

    /// Build from a declarative [`ExtractionSpec`] with SchemaJson's default execution
    /// params. Run the *same* spec through [`ToolCallExtractor::with_spec`] to
    /// compare mechanisms.
    pub fn with_spec(backend: Arc<dyn LlmBackend>, spec: ExtractionSpec) -> Self {
        let mut config = Self::default_config();
        config.spec = spec;
        Self::with_config(backend, config)
    }

    pub fn schema_mode(mut self, mode: SchemaMode) -> Self {
        self.config.spec.mode = mode;
        self
    }

    pub fn config(&self) -> &ExtractionConfig {
        &self.config
    }

    /// System prompt for the schema-driven (non-template) path: the extraction
    /// instructions + the seed schema. The schema is small, so it is pushed here
    /// once as configuration; the user turn carries only the document text. Mode
    /// shapes the wording (Open infers, Fixed closes, Evolving may extend).
    fn build_system_prompt(&self) -> String {
        let schema = serde_json::json!({
            "nodes": self.config.entity_types_list(),
            "relations": self.config.predicates_list(),
            "attributes": self.config.attributes_list(),
        });
        let schema_json = serde_json::to_string(&schema).unwrap_or_default();

        match self.config.spec.mode {
            SchemaMode::Open => format!(
                "Extract entities and relationships from the user's text.\n\
No predefined schema is given — infer suitable entity types and relation types from the content.\n\n\
Output JSON with:\n\
1. \"entities\": {{\"entity_name\": {{\"type\": \"EntityType\", \"attributes\": {{\"attr\": \"value\"}}}}}}\n\
2. \"relationships\": [[\"subject\", \"predicate\", \"object\"]]\n\
3. \"entity_types\": {{\"entity_name\": \"type\"}} (map entities to their types)\n\n\
{DIRECTION_RULE}\n\
Ensure valid JSON output."
            ),
            SchemaMode::Fixed => format!(
                "Extract entities and relationships from the user's text using the provided schema.\n\n\
Schema:\n{schema_json}\n\n\
Output JSON with:\n\
1. \"entities\": {{\"entity_name\": {{\"type\": \"EntityType\", \"attributes\": {{\"attr\": \"value\"}}}}}}\n\
2. \"relationships\": [[\"subject\", \"predicate\", \"object\"]]\n\
3. \"entity_types\": {{\"entity_name\": \"type\"}} (map entities to their types)\n\n\
Use only the entity types and relations from the schema.\n\
{DIRECTION_RULE}\n\
Ensure valid JSON output."
            ),
            SchemaMode::Evolving => format!(
                "Extract entities and relationships from the user's text using the provided schema as guidance.\n\
You may suggest new entity types, relations, or attributes if they better represent the content.\n\n\
Schema:\n{schema_json}\n\n\
Output JSON with:\n\
1. \"entities\": {{\"entity_name\": {{\"type\": \"EntityType\", \"attributes\": {{\"attr\": \"value\"}}}}}}\n\
2. \"relationships\": [[\"subject\", \"predicate\", \"object\"]]\n\
3. \"entity_types\": {{\"entity_name\": \"type\"}} (map entities to their types)\n\
4. \"new_schema_types\": {{\"nodes\": [], \"relations\": [], \"attributes\": []}} (if suggesting new types)\n\n\
{DIRECTION_RULE}\n\
Ensure valid JSON output."
            ),
        }
    }

    fn build_graph(&self, data: &serde_json::Value) -> KnowledgeGraph {
        let entity_types = data.get("entity_types").and_then(|v| v.as_object());
        // Honour the configured merge strategy on same-name collisions within the
        // response. When dedup is disabled, keep the historical first-wins.
        let strategy = if self.config.spec.merge_duplicates {
            self.config.spec.merge_strategy
        } else {
            MergeStrategy::KeepExisting
        };
        let mut gb = GraphBuilder::new().merge_strategy(strategy);

        if let Some(obj) = data.get("entities").and_then(|v| v.as_object()) {
            for (name, info) in obj {
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
                    let attrs = collect_attributes(
                        io,
                        &["type", "label", "name", "description", "attributes"],
                    );
                    (t, attrs)
                } else {
                    let t = entity_types
                        .and_then(|et| et.get(name))
                        .and_then(|v| v.as_str())
                        .unwrap_or("UNKNOWN")
                        .to_string();
                    (t, HashMap::new())
                };

                // SchemaJson uses strict name match, fallback PHYSICAL_OBJECT (its own
                // quirk, distinct from `from_loose`), so parsing stays here rather
                // than in the shared GraphBuilder.
                let entity_type = type_str
                    .to_uppercase()
                    .replace([' ', '-'], "_")
                    .parse::<EntityType>()
                    .unwrap_or(EntityType::PhysicalObject);
                let description = attributes
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                // GraphBuilder keys by lowercased name, so a relationship that
                // references the entity with different casing still resolves.
                gb.add_entity_with_raw_type(
                    name,
                    entity_type,
                    Some(type_str),
                    description,
                    attributes,
                );
            }
        }

        if let Some(rels) = data.get("relationships").and_then(|v| v.as_array()) {
            for rel in rels {
                let Some((subject_name, predicate_str, object_name, attributes)) =
                    relation_parts(rel)
                else {
                    continue;
                };

                let predicate_type = predicate_str
                    .to_uppercase()
                    .replace([' ', '-'], "_")
                    .parse::<PredicateType>()
                    .unwrap_or(PredicateType::RelatedTo);
                let predicate = Predicate::with_label(predicate_type, predicate_str.clone());
                gb.add_relation(&subject_name, predicate, &object_name, |t| {
                    t.metadata = attributes;
                });
            }
        }

        gb.into_graph()
    }

    /// Prune a parsed response to the seed schema — `Fixed` mode's hard
    /// guarantee. Until now `Fixed` only *asked* the model to stay in-schema (a
    /// soft prompt constraint); this drops what slips through, so the engine
    /// gives the same closed-world result as ToolCall's enum-constrained args.
    ///
    /// An entity is dropped when its type is outside the schema; a relation when
    /// its predicate is outside the schema *or* an endpoint entity was itself
    /// dropped. A relation to a genuinely undeclared entity is left to the usual
    /// dangling-endpoint drop in [`build_graph`]. A schema half left empty (no
    /// node or no relation types) leaves that half unconstrained. Returns the
    /// pruned data and what was removed.
    fn enforce_fixed(&self, data: &serde_json::Value) -> (serde_json::Value, FixedDrops) {
        let nodes: HashSet<String> = self
            .config
            .entity_types_list()
            .iter()
            .map(|s| norm_type(s))
            .collect();
        let rels: HashSet<String> = self
            .config
            .predicates_list()
            .iter()
            .map(|s| norm_type(s))
            .collect();
        let entity_types = data.get("entity_types").and_then(|v| v.as_object());
        let mut drops = FixedDrops::default();

        // Entities: keep those whose (resolved) type is in the node schema.
        let mut kept_entities = serde_json::Map::new();
        let mut dropped_names: HashSet<String> = HashSet::new();
        if let Some(obj) = data.get("entities").and_then(|v| v.as_object()) {
            for (name, info) in obj {
                let type_str = info
                    .as_object()
                    .and_then(|io| io.get("type").and_then(|v| v.as_str()))
                    .map(String::from)
                    .or_else(|| {
                        entity_types
                            .and_then(|et| et.get(name))
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_else(|| "UNKNOWN".into());
                let t = norm_type(&type_str);
                if nodes.is_empty() || nodes.contains(&t) {
                    kept_entities.insert(name.clone(), info.clone());
                } else {
                    drops.records += 1;
                    drops.types.insert(t);
                    dropped_names.insert(name.to_lowercase());
                }
            }
        }

        // Relationships: keep those with an in-schema predicate and both
        // endpoints surviving.
        let mut kept_rels = Vec::new();
        if let Some(arr) = data.get("relationships").and_then(|v| v.as_array()) {
            for rel in arr {
                let Some((s, p, o, _)) = relation_parts(rel) else {
                    continue;
                };
                let pt = norm_type(&p);
                let pred_ok = rels.is_empty() || rels.contains(&pt);
                let endpoint_dropped = dropped_names.contains(&s.to_lowercase())
                    || dropped_names.contains(&o.to_lowercase());
                if pred_ok && !endpoint_dropped {
                    kept_rels.push(rel.clone());
                } else {
                    drops.records += 1;
                    if !pred_ok {
                        drops.types.insert(pt);
                    }
                }
            }
        }

        let mut out = data.clone();
        if let Some(m) = out.as_object_mut() {
            m.insert("entities".into(), serde_json::Value::Object(kept_entities));
            m.insert("relationships".into(), serde_json::Value::Array(kept_rels));
        }
        (out, drops)
    }
}

fn collect_attributes(
    obj: &serde_json::Map<String, serde_json::Value>,
    reserved: &[&str],
) -> HashMap<String, serde_json::Value> {
    let mut attrs: HashMap<String, serde_json::Value> = obj
        .get("attributes")
        .and_then(|v| v.as_object())
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    for (key, value) in obj {
        if reserved.iter().any(|r| key == r) {
            continue;
        }
        attrs.entry(key.clone()).or_insert_with(|| value.clone());
    }
    attrs
}

fn relation_parts(
    rel: &serde_json::Value,
) -> Option<(String, String, String, HashMap<String, serde_json::Value>)> {
    if let Some(arr) = rel.as_array() {
        if arr.len() < 3 {
            return None;
        }
        let source = arr[0].as_str().unwrap_or_default().to_string();
        let predicate = arr[1].as_str().unwrap_or_default().to_string();
        let target = arr[2].as_str().unwrap_or_default().to_string();
        let attributes = arr
            .get(3)
            .and_then(|v| v.as_object())
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        return Some((source, predicate, target, attributes));
    }

    let obj = rel.as_object()?;
    let source = obj
        .get("source")
        .or_else(|| obj.get("subject"))
        .and_then(|v| v.as_str())?
        .to_string();
    let target = obj
        .get("target")
        .or_else(|| obj.get("object"))
        .and_then(|v| v.as_str())?
        .to_string();
    let predicate = obj
        .get("type")
        .or_else(|| obj.get("predicate"))
        .or_else(|| obj.get("relation"))
        .and_then(|v| v.as_str())?
        .to_string();
    let attributes = collect_attributes(
        obj,
        &[
            "source",
            "subject",
            "target",
            "object",
            "type",
            "predicate",
            "relation",
            "attributes",
        ],
    );
    Some((source, predicate, target, attributes))
}

#[async_trait::async_trait]
impl Extractor for SchemaJsonExtractor {
    async fn extract(&self, text: &str) -> anyhow::Result<ExtractionResponse> {
        validate_input(text, self.config.min_segment_size, self.quiet)?;

        // Fixed/Evolving extract against a seed schema; constraining to (or
        // evolving from) an empty schema is the degenerate cell of the grid.
        // A template (preset) renders the prompt from its own guideline + fields
        // and ignores the schema entirely, so the schema requirement doesn't
        // apply when one is attached (`--preset X --schema-mode fixed` is valid).
        if self.config.spec.template.is_none()
            && self.config.spec.mode.needs_schema()
            && self.config.spec.schema.is_empty()
        {
            anyhow::bail!(
                "schema mode {:?} requires a non-empty schema (seed one via \
                 ExtractionConfig::from_schema; CLI: --schema <file>), or use SchemaMode::Open",
                self.config.spec.mode
            );
        }

        let opts = CompletionOptions {
            model: self.config.model_name.clone(),
            temperature: 0.3,
            max_tokens: 4000,
        };
        // Schema path: instructions + schema in the system turn, text in the user
        // turn. Template path: the preset renders one self-contained prompt.
        let call = if let Some(tpl) = &self.config.spec.template {
            let lang = tpl.resolve_lang(self.config.spec.language.as_deref());
            let prompt = crate::template::render_prompt(tpl, &lang, text);
            self.backend.complete_prompt(&prompt, &opts).await
        } else {
            let messages = [
                Message::system(self.build_system_prompt()),
                Message::user(format!("Text:\n{text}")),
            ];
            self.backend.complete(&messages, &opts).await
        };
        let response = match call {
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

        // Fixed mode is now hard: drop whatever the model emitted outside the
        // schema instead of only asking it to comply. No-op for Open/Evolving,
        // or when the schema is empty (e.g. a template-driven Fixed run).
        let fixed_drops =
            if self.config.spec.mode == SchemaMode::Fixed && !self.config.spec.schema.is_empty() {
                Some(self.enforce_fixed(&data))
            } else {
                None
            };
        let data = match &fixed_drops {
            Some((filtered, _)) => filtered,
            None => &data,
        };

        let mut kg = self.build_graph(data);
        // Single-shot over the whole text, so provenance is whole-document.
        crate::citation::stamp_whole_document(&mut kg, &self.config.source_doc, text);

        let mut resp = ExtractionResponse::new(kg);
        resp.metadata
            .insert("model".into(), serde_json::json!(self.config.model_name));
        resp.metadata
            .insert("mode".into(), serde_json::json!("schema_json"));
        resp.metadata.insert(
            "schema_mode".into(),
            serde_json::json!(self.config.spec.mode.as_str()),
        );
        resp.metadata.insert(
            "schema_used".into(),
            serde_json::json!({
                "entity_types": self.config.entity_types_list(),
                "predicates": self.config.predicates_list(),
                "attributes": self.config.attributes_list(),
            }),
        );
        if let Some(new_schema) = data.get("new_schema_types") {
            resp.metadata
                .insert("new_schema_types".into(), new_schema.clone());
        }
        if let Some((_, drops)) = &fixed_drops {
            if !self.quiet && drops.records > 0 {
                let csv = drops.types.iter().cloned().collect::<Vec<_>>().join(", ");
                eprintln!(
                    "schema-json: dropped {} out-of-schema record(s){}",
                    drops.records,
                    if csv.is_empty() {
                        String::new()
                    } else {
                        format!(": {csv}")
                    }
                );
            }
            resp.metadata.insert(
                "schema_dropped_records".into(),
                serde_json::json!(drops.records),
            );
            resp.metadata.insert(
                "schema_dropped_types".into(),
                serde_json::json!(drops.types.iter().cloned().collect::<Vec<_>>()),
            );
        }
        resp.config = Some(self.config.clone());
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MockBackend;
    use crate::graph_build::entity_id;

    #[tokio::test]
    async fn schema_based_extraction() {
        let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"},
                                      "GPT-4": {"type": "TECHNOLOGY"}},
                        "relationships": [["OpenAI", "DEVELOPED_BY", "GPT-4"]]}"#;
        let backend = Arc::new(MockBackend::single(json));
        let ex = SchemaJsonExtractor::new(backend);
        let out = ex.extract("OpenAI developed GPT-4.").await.unwrap();
        assert_eq!(out.num_entities(), 2);
        assert_eq!(out.num_triples(), 1);
    }

    #[tokio::test]
    async fn open_schema_output_preserves_raw_entity_and_relation_types() {
        let json = r#"{"entities": {
            "KG-RAG": {"type": "METHOD"},
            "RAG": {"type": "FRAMEWORK"}
        }, "relationships": [["KG-RAG", "BUILDS_ON", "RAG"]]}"#;
        let out = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
            .extract("KG-RAG builds on RAG.")
            .await
            .unwrap();

        let doc = out.knowledge_graph.to_dict();
        let kg_rag_id = entity_id("KG-RAG");
        assert_eq!(doc["entities"][&kg_rag_id]["type"], "METHOD");
        assert_eq!(
            doc["entities"][&kg_rag_id]["normalized_type"],
            "PHYSICAL_OBJECT"
        );
        assert_eq!(doc["triples"][0]["predicate"]["type"], "BUILDS_ON");
        assert_eq!(
            doc["triples"][0]["predicate"]["normalized_type"],
            "RELATED_TO"
        );
    }

    #[tokio::test]
    async fn object_relationships_preserve_attributes_as_triple_metadata() {
        let json = r#"{"entities": {
            "KG-RAG": {"type": "METHOD", "evidence_quote": "KG-RAG framework"},
            "SPOKE": {"type": "KNOWLEDGE_GRAPH", "attributes": {"role": "knowledge source"}}
        }, "relationships": [{
            "source": "KG-RAG",
            "type": "RETRIEVES_FROM",
            "target": "SPOKE",
            "evidence_quote": "retrieves context from SPOKE",
            "source_section": "Methods"
        }]}"#;
        let out = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
            .extract("KG-RAG retrieves context from SPOKE.")
            .await
            .unwrap();

        let doc = out.knowledge_graph.to_dict();
        assert_eq!(doc["triples"][0]["predicate"]["type"], "RETRIEVES_FROM");
        assert_eq!(
            doc["triples"][0]["metadata"]["evidence_quote"],
            "retrieves context from SPOKE"
        );
        assert_eq!(doc["triples"][0]["metadata"]["source_section"], "Methods");
        let kg_rag_id = entity_id("KG-RAG");
        assert_eq!(
            doc["entities"][&kg_rag_id]["metadata"]["evidence_quote"],
            "KG-RAG framework"
        );
    }

    /// Single-shot engines never chunk, so pre-chunked input goes through the
    /// trait's default: the chunk texts are joined and extracted in one call,
    /// exactly like plain-text input.
    #[tokio::test]
    async fn prechunked_default_joins_chunks_into_one_call() {
        use crate::chunking::Segment;
        let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"}}, "relationships": []}"#;
        let backend = Arc::new(MockBackend::single(json));
        let ex = SchemaJsonExtractor::new(backend.clone());

        let chunks = vec![
            Segment {
                content: "OpenAI is an AI lab.".into(),
                index: 0,
                start: 0,
                end: 20,
                lines: Some((1, 1)),
            },
            Segment {
                content: "It developed GPT-4.".into(),
                index: 1,
                start: 20,
                end: 39,
                lines: Some((2, 2)),
            },
        ];
        let out = ex.extract_prechunked(&chunks).await.unwrap();
        assert_eq!(out.num_entities(), 1);

        let prompts = backend.seen_prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1, "single-shot: exactly one LLM call");
        assert!(
            prompts[0].contains("OpenAI is an AI lab.\n\nIt developed GPT-4."),
            "chunks must be joined into the user turn: {}",
            prompts[0]
        );
    }

    #[tokio::test]
    async fn within_response_duplicates_honor_merge_strategy() {
        // The model emits the same entity twice (different casing) with different
        // descriptions. The configured merge_strategy must govern how they fold —
        // not a hardcoded first-wins.
        let json = r#"{"entities": {
            "OpenAI": {"type": "ORGANIZATION", "attributes": {"description": "first"}},
            "openai": {"type": "ORGANIZATION", "attributes": {"description": "second"}}
        }, "relationships": []}"#;
        let desc_of = |r: &ExtractionResponse| {
            r.knowledge_graph
                .entities
                .values()
                .next()
                .unwrap()
                .description
                .clone()
        };

        // Default KeepExisting: first occurrence wins (historical behaviour).
        let keep = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
            .extract("OpenAI")
            .await
            .unwrap();
        assert_eq!(keep.num_entities(), 1);
        assert_eq!(desc_of(&keep).as_deref(), Some("first"));

        // KeepIncoming: the later duplicate's data must replace it — proving the
        // strategy is actually applied in the schema-json path.
        let spec = ExtractionSpec {
            merge_strategy: MergeStrategy::KeepIncoming,
            ..Default::default()
        };
        let inc = SchemaJsonExtractor::with_spec(Arc::new(MockBackend::single(json)), spec)
            .extract("OpenAI")
            .await
            .unwrap();
        assert_eq!(inc.num_entities(), 1);
        assert_eq!(
            desc_of(&inc).as_deref(),
            Some("second"),
            "merge_strategy must take effect on within-response duplicates"
        );
    }

    #[tokio::test]
    async fn template_extracts_under_fixed_mode_without_a_schema() {
        // A preset drives the prompt itself, so `--schema-mode fixed` (which
        // otherwise demands a non-empty schema) must NOT reject a template-only
        // spec with an empty schema — the template path ignores the schema.
        use crate::template::gallery;
        let tpl = gallery::get("general/concept_graph").expect("concept_graph preset");
        let spec = ExtractionSpec::from_template(tpl, Some("en".into()));
        let json = r#"{"entities": {"Photosynthesis": {"type": "PROCESS"}}, "relationships": []}"#;
        let out = SchemaJsonExtractor::with_spec(Arc::new(MockBackend::single(json)), spec)
            .schema_mode(SchemaMode::Fixed)
            .extract("Photosynthesis is a process.")
            .await
            .expect("template-driven extraction must succeed despite Fixed mode + empty schema");
        assert_eq!(out.num_entities(), 1);
    }

    #[tokio::test]
    async fn relationship_resolves_case_insensitively() {
        // Entities are "OpenAI"/"GPT-4" but the relationship references them in a
        // different case; the edge must still be created, not silently dropped.
        let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"}, "GPT-4": {"type": "TECHNOLOGY"}},
                       "relationships": [["openai", "uses", "gpt-4"]]}"#;
        let out = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
            .extract("text")
            .await
            .unwrap();
        assert_eq!(out.num_entities(), 2);
        assert_eq!(
            out.num_triples(),
            1,
            "relationship must resolve despite case mismatch"
        );
    }

    #[tokio::test]
    async fn entity_ids_are_deterministic_md5() {
        let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"}}, "relationships": []}"#;
        let a = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
            .extract("text")
            .await
            .unwrap();
        let b = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
            .extract("text")
            .await
            .unwrap();
        let ka: Vec<&String> = a.knowledge_graph.entities.keys().collect();
        let kb: Vec<&String> = b.knowledge_graph.entities.keys().collect();
        assert_eq!(
            ka, kb,
            "SchemaJson entity ids must be deterministic across runs"
        );
        let expected = entity_id("OpenAI");
        assert!(
            a.knowledge_graph.entities.contains_key(&expected),
            "id must follow the shared md5(name) scheme"
        );
    }

    #[tokio::test]
    async fn open_is_the_default_and_needs_no_schema() {
        // Default mode is Open: an empty schema is fine, model extracts freely.
        let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"}}, "relationships": []}"#;
        let out = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
            .extract("text")
            .await
            .unwrap();
        assert_eq!(out.metadata["schema_mode"], serde_json::json!("open"));
        assert_eq!(out.metadata["mode"], serde_json::json!("schema_json"));
        assert_eq!(out.num_entities(), 1);
    }

    #[tokio::test]
    async fn evolving_mode_captures_new_schema() {
        let json = r#"{"entities": {"Movie X": {"type": "WORK_OF_ART"}},
                        "relationships": [],
                        "new_schema_types": {"nodes": ["Movie"], "relations": ["starring"], "attributes": []}}"#;
        // Evolving requires a non-empty seed schema.
        let cfg = ExtractionConfig::from_schema(Schema::new(
            vec!["WORK_OF_ART".into()],
            vec!["RELATED_TO".into()],
            vec![],
        ));
        let ex = SchemaJsonExtractor::with_config(Arc::new(MockBackend::single(json)), cfg)
            .schema_mode(SchemaMode::Evolving);
        let out = ex.extract("Some text about a movie.").await.unwrap();
        assert!(out.metadata.contains_key("new_schema_types"));
        assert_eq!(out.metadata["schema_mode"], serde_json::json!("evolving"));
    }

    #[tokio::test]
    async fn fixed_mode_drops_out_of_schema_records() {
        // Schema allows ORGANIZATION nodes and DEVELOPED_BY relations only. The
        // model leaks a TECHNOLOGY entity and a USES relation. Fixed mode must now
        // hard-drop them (not just prompt against them), and the DEVELOPED_BY
        // relation must also go because its GPT-4 endpoint was dropped.
        let json = r#"{"entities": {
            "OpenAI": {"type": "ORGANIZATION"},
            "GPT-4": {"type": "TECHNOLOGY"}
        }, "relationships": [
            ["OpenAI", "DEVELOPED_BY", "GPT-4"],
            ["OpenAI", "USES", "OpenAI"]
        ]}"#;
        let cfg = ExtractionConfig::from_schema(Schema::new(
            vec!["ORGANIZATION".into()],
            vec!["DEVELOPED_BY".into()],
            vec![],
        ));
        let out = SchemaJsonExtractor::with_config(Arc::new(MockBackend::single(json)), cfg)
            .schema_mode(SchemaMode::Fixed)
            .extract("text")
            .await
            .unwrap();
        assert_eq!(
            out.num_entities(),
            1,
            "only the ORGANIZATION entity survives"
        );
        assert_eq!(
            out.num_triples(),
            0,
            "USES is out-of-schema; DEVELOPED_BY loses its endpoint"
        );
        // 1 entity (TECHNOLOGY) + 2 relations (USES type, DEVELOPED_BY endpoint).
        assert_eq!(out.metadata["schema_dropped_records"], serde_json::json!(3));
        let dropped = out.metadata["schema_dropped_types"].as_array().unwrap();
        assert!(dropped.contains(&serde_json::json!("TECHNOLOGY")));
        assert!(dropped.contains(&serde_json::json!("USES")));
    }

    #[tokio::test]
    async fn fixed_mode_keeps_in_schema_records() {
        // Everything is in-schema → nothing dropped, the drop metadata reports 0.
        let json = r#"{"entities": {"OpenAI": {"type": "ORGANIZATION"}, "GPT-4": {"type": "TECHNOLOGY"}},
                       "relationships": [["OpenAI", "DEVELOPED_BY", "GPT-4"]]}"#;
        let cfg = ExtractionConfig::from_schema(Schema::new(
            vec!["ORGANIZATION".into(), "TECHNOLOGY".into()],
            vec!["DEVELOPED_BY".into()],
            vec![],
        ));
        let out = SchemaJsonExtractor::with_config(Arc::new(MockBackend::single(json)), cfg)
            .schema_mode(SchemaMode::Fixed)
            .extract("text")
            .await
            .unwrap();
        assert_eq!(out.num_entities(), 2);
        assert_eq!(out.num_triples(), 1);
        assert_eq!(out.metadata["schema_dropped_records"], serde_json::json!(0));
    }

    #[tokio::test]
    async fn open_mode_does_not_enforce_schema() {
        // The same leak under Open mode must pass through untouched (no drop
        // metadata at all) — enforcement is Fixed-only.
        let json = r#"{"entities": {"GPT-4": {"type": "TECHNOLOGY"}}, "relationships": []}"#;
        let out = SchemaJsonExtractor::new(Arc::new(MockBackend::single(json)))
            .extract("text")
            .await
            .unwrap();
        assert_eq!(out.num_entities(), 1);
        assert!(!out.metadata.contains_key("schema_dropped_records"));
    }

    #[tokio::test]
    async fn fixed_mode_without_schema_errors() {
        // Fixed on an empty schema is the degenerate combo — must error, not
        // silently tell the model to "use only types from []".
        let err = SchemaJsonExtractor::new(Arc::new(MockBackend::single("{}")))
            .schema_mode(SchemaMode::Fixed)
            .extract("text")
            .await;
        assert!(err.is_err(), "Fixed mode with an empty schema must error");
    }

    #[test]
    fn one_spec_runs_through_both_engines() {
        use crate::extractor::ToolCallExtractor;
        // A single declarative spec configures either mechanism (with_spec) —
        // the spec/execution split: define the contract once, pick the executor.
        let spec = ExtractionSpec::new(
            Schema::new(
                vec!["ORGANIZATION".into()],
                vec!["DEVELOPED_BY".into()],
                vec![],
            ),
            SchemaMode::Fixed,
        );
        let sj = SchemaJsonExtractor::with_spec(Arc::new(MockBackend::single("{}")), spec.clone());
        let tool = ToolCallExtractor::with_spec(Arc::new(MockBackend::single("{}")), spec.clone());
        assert_eq!(
            sj.config().spec,
            spec,
            "SchemaJson must carry the spec verbatim"
        );
        assert_eq!(
            tool.config().spec,
            spec,
            "ToolCall must carry the same spec"
        );
        // Execution params stay engine-specific (both default to qwen-max here,
        // but the segment sizes differ: 3000 vs 5000).
        assert_eq!(sj.config().segment_size, 3000);
        assert_eq!(tool.config().segment_size, 5000);
    }
}
