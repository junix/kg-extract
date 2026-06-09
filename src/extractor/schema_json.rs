//! SchemaJsonExtractor — schema-driven extraction with three schema modes:
//! `Open` (no predefined types), `Fixed` (closed schema), and `Evolving`
//! (seed schema the model may extend). Ported from `graph/kg_extractor/youtu.py`.

use super::{validate_input, Extractor, SchemaMode};
use crate::backend::{CompletionOptions, LlmBackend};
use crate::graph_build::GraphBuilder;
use crate::parser::extract_json_from_response;
use crate::types::{
    EntityType, ExtractionConfig, ExtractionResponse, ExtractionSpec, KnowledgeGraph, MergeStrategy,
    Predicate, PredicateType, Schema,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Schema-based knowledge graph extractor.
pub struct SchemaJsonExtractor {
    backend: Arc<dyn LlmBackend>,
    config: ExtractionConfig,
    pub quiet: bool,
}

impl SchemaJsonExtractor {
    pub fn default_config() -> ExtractionConfig {
        // SchemaJson base config starts from an EMPTY schema (no default seeding).
        ExtractionConfig {
            spec: ExtractionSpec { schema: Schema::default(), ..Default::default() },
            model_name: "qwen-max".into(),
            segment_size: 3000,
            min_segment_size: 100,
            ..Default::default()
        }
    }

    pub fn new(backend: Arc<dyn LlmBackend>) -> Self {
        SchemaJsonExtractor { backend, config: Self::default_config(), quiet: false }
    }

    pub fn with_config(backend: Arc<dyn LlmBackend>, config: ExtractionConfig) -> Self {
        SchemaJsonExtractor { backend, config, quiet: false }
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

    fn build_prompt(&self, text: &str) -> String {
        // A rich template (preset), when attached, drives the prompt directly —
        // its guideline + output fields replace the bare type-vocabulary, and the
        // output contract stays the same JSON shape `build_graph` parses.
        if let Some(tpl) = &self.config.spec.template {
            let lang = tpl.resolve_lang(self.config.spec.language.as_deref());
            return crate::template::render_prompt(tpl, &lang, text);
        }

        let schema = serde_json::json!({
            "nodes": self.config.entity_types_list(),
            "relations": self.config.predicates_list(),
            "attributes": self.config.attributes_list(),
        });
        let schema_json = serde_json::to_string(&schema).unwrap_or_default();

        match self.config.spec.mode {
            SchemaMode::Open => format!(
                "Extract entities and relationships from the following text.\n\
No predefined schema is given — infer suitable entity types and relation types from the content.\n\n\
Text:\n{text}\n\n\
Output JSON with:\n\
1. \"entities\": {{\"entity_name\": {{\"type\": \"EntityType\", \"attributes\": {{\"attr\": \"value\"}}}}}}\n\
2. \"relationships\": [[\"subject\", \"predicate\", \"object\"]]\n\
3. \"entity_types\": {{\"entity_name\": \"type\"}} (map entities to their types)\n\n\
Ensure valid JSON output."
            ),
            SchemaMode::Fixed => format!(
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
            SchemaMode::Evolving => format!(
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

                // SchemaJson uses strict name match, fallback PHYSICAL_OBJECT (its own
                // quirk, distinct from `from_loose`), so parsing stays here rather
                // than in the shared GraphBuilder.
                let entity_type = type_str
                    .to_uppercase()
                    .replace([' ', '-'], "_")
                    .parse::<EntityType>()
                    .unwrap_or(EntityType::PhysicalObject);
                let description =
                    attributes.get("description").and_then(|v| v.as_str()).map(String::from);
                // GraphBuilder keys by lowercased name, so a relationship that
                // references the entity with different casing still resolves.
                gb.add_entity(name, entity_type, description, attributes);
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

                let predicate_type = predicate_str
                    .to_uppercase()
                    .replace([' ', '-'], "_")
                    .parse::<PredicateType>()
                    .unwrap_or(PredicateType::RelatedTo);
                let predicate = Predicate::with_label(predicate_type, predicate_str);
                gb.add_relation(subject_name, predicate, object_name, |_| {});
            }
        }

        gb.into_graph()
    }
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

        let kg = self.build_graph(&data);

        let mut resp = ExtractionResponse::new(kg);
        resp.metadata.insert("model".into(), serde_json::json!(self.config.model_name));
        resp.metadata.insert("mode".into(), serde_json::json!("schema_json"));
        resp.metadata.insert("schema_mode".into(), serde_json::json!(self.config.spec.mode.as_str()));
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
    async fn within_response_duplicates_honor_merge_strategy() {
        // The model emits the same entity twice (different casing) with different
        // descriptions. The configured merge_strategy must govern how they fold —
        // not a hardcoded first-wins.
        let json = r#"{"entities": {
            "OpenAI": {"type": "ORGANIZATION", "attributes": {"description": "first"}},
            "openai": {"type": "ORGANIZATION", "attributes": {"description": "second"}}
        }, "relationships": []}"#;
        let desc_of = |r: &ExtractionResponse| {
            r.knowledge_graph.entities.values().next().unwrap().description.clone()
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
        let spec = ExtractionSpec { merge_strategy: MergeStrategy::KeepIncoming, ..Default::default() };
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
        assert_eq!(out.num_triples(), 1, "relationship must resolve despite case mismatch");
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
        assert_eq!(ka, kb, "SchemaJson entity ids must be deterministic across runs");
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
            Schema::new(vec!["ORGANIZATION".into()], vec!["DEVELOPED_BY".into()], vec![]),
            SchemaMode::Fixed,
        );
        let sj = SchemaJsonExtractor::with_spec(Arc::new(MockBackend::single("{}")), spec.clone());
        let tool = ToolCallExtractor::with_spec(Arc::new(MockBackend::single("{}")), spec.clone());
        assert_eq!(sj.config().spec, spec, "SchemaJson must carry the spec verbatim");
        assert_eq!(tool.config().spec, spec, "ToolCall must carry the same spec");
        // Execution params stay engine-specific (both default to qwen-max here,
        // but the segment sizes differ: 3000 vs 5000).
        assert_eq!(sj.config().segment_size, 3000);
        assert_eq!(tool.config().segment_size, 5000);
    }
}
