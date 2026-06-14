//! ToolCallExtractor — extraction via LLM **tool / function calling**.
//!
//! Instead of asking the model to emit a delimiter- or JSON-formatted blob that
//! we then parse, we expose typed tools and let the model call them; the call
//! arguments are already structured, so parsing is essentially free.
//!
//! Tools:
//! - `add_entity(name, type, description, attributes)`
//! - `add_relation(source, predicate, target, description, strength)`
//! - `add_attribute(entity, key, value)`
//! - `propose_schema_type(kind, name, reason)` — schema evolution (agent-ish)
//! - `list_entities()` — read tool (only meaningful in multi-round mode)
//! - `finish()` — explicit termination signal
//!
//! Default execution is **single-round collection**: one request, gather every
//! tool call the model made in that response, build the graph. Set
//! `max_rounds > 1` for a bounded agentic loop where tool results (including
//! `list_entities`) are fed back so the model can avoid dangling relations.

use super::{validate_input, Extractor, SchemaMode};
use crate::backend::{CompletionOptions, LlmBackend, Message, ToolInvocation, ToolSpec};
use crate::graph_build::{build_predicate, parse_entity_type, GraphBuilder};
use crate::merger::dedup_graph;
use crate::types::{
    ExtractionConfig, ExtractionResponse, ExtractionSpec, KnowledgeGraph, MergeStrategy, Schema,
};
use std::collections::HashMap;
use std::sync::Arc;

const SYSTEM_PROMPT: &str = "You are a knowledge-graph extraction engine. Read the text and call the provided tools to record every entity, relationship and salient attribute. Call add_entity before referencing an entity in add_relation. Call finish when done.";

/// Knowledge graph extractor driven by LLM tool calls.
pub struct ToolCallExtractor {
    backend: Arc<dyn LlmBackend>,
    config: ExtractionConfig,
    /// Max tool-calling rounds. 1 = single-round collection (default).
    pub max_rounds: usize,
    pub quiet: bool,
}

impl ToolCallExtractor {
    pub fn default_config() -> ExtractionConfig {
        // Like SchemaJson, start from an EMPTY schema (no default seeding): the
        // default `Open` mode applies no enum constraints, and `Fixed`/`Evolving`
        // take an explicit schema.
        ExtractionConfig {
            spec: ExtractionSpec {
                schema: Schema::default(),
                ..Default::default()
            },
            model_name: "qwen-max".into(),
            segment_size: 5000,
            min_segment_size: 100,
            ..Default::default()
        }
    }

    pub fn new(backend: Arc<dyn LlmBackend>) -> Self {
        ToolCallExtractor {
            backend,
            config: Self::default_config(),
            max_rounds: 1,
            quiet: false,
        }
    }

    pub fn with_config(backend: Arc<dyn LlmBackend>, config: ExtractionConfig) -> Self {
        ToolCallExtractor {
            backend,
            config,
            max_rounds: 1,
            quiet: false,
        }
    }

    /// Build from a declarative [`ExtractionSpec`] with ToolCall's default
    /// execution params. The same spec also runs through
    /// [`SchemaJsonExtractor::with_spec`](super::SchemaJsonExtractor::with_spec).
    pub fn with_spec(backend: Arc<dyn LlmBackend>, spec: ExtractionSpec) -> Self {
        let mut config = Self::default_config();
        config.spec = spec;
        Self::with_config(backend, config)
    }

    pub fn schema_mode(mut self, mode: SchemaMode) -> Self {
        self.config.spec.mode = mode;
        self
    }

    pub fn max_rounds(mut self, rounds: usize) -> Self {
        self.max_rounds = rounds.max(1);
        self
    }

    pub fn config(&self) -> &ExtractionConfig {
        &self.config
    }

    /// System prompt = the engine's role + the seed schema. The schema is small
    /// (a handful of type names in practice), so it is pushed here once as
    /// configuration rather than repeated in the user turn or pulled via a tool.
    /// Mode shapes the wording: Fixed = closed list, Evolving = seed + propose,
    /// Open/empty = no schema block.
    fn system_prompt(&self) -> String {
        if self.config.spec.schema.is_empty() {
            return SYSTEM_PROMPT.to_string();
        }
        let entities = self.config.entity_types_list().join(", ");
        let relations = self.config.predicates_list().join(", ");
        let attributes = self.config.attributes_list().join(", ");
        let block = match self.config.spec.mode {
            SchemaMode::Fixed => format!(
                "\n\nSCHEMA (closed — use ONLY these types; anything else is discarded):\n\
                 entity types: {entities}\nrelation types: {relations}\nattributes: {attributes}"
            ),
            SchemaMode::Evolving => format!(
                "\n\nSCHEMA (seed — prefer these; call propose_schema_type for a genuinely new type):\n\
                 entity types: {entities}\nrelation types: {relations}\nattributes: {attributes}"
            ),
            SchemaMode::Open => format!(
                "\n\nSCHEMA (hints — name types freely):\n\
                 entity types: {entities}\nrelation types: {relations}\nattributes: {attributes}"
            ),
        };
        format!("{SYSTEM_PROMPT}{block}")
    }

    /// JSON-Schema tool definitions. Only `Fixed` mode enum-constrains the entity
    /// type / predicate args to the seeded schema; `Open`/`Evolving` leave them
    /// free-form so the model can name (or propose) new types.
    fn tools(&self) -> Vec<ToolSpec> {
        let enforce = matches!(self.config.spec.mode, SchemaMode::Fixed);
        let type_schema = if enforce && !self.config.entity_types_list().is_empty() {
            serde_json::json!({"type": "string", "enum": self.config.entity_types_list()})
        } else {
            serde_json::json!({"type": "string"})
        };
        let predicate_schema = if enforce && !self.config.predicates_list().is_empty() {
            serde_json::json!({"type": "string", "enum": self.config.predicates_list()})
        } else {
            serde_json::json!({"type": "string"})
        };

        let mut tools = vec![
            ToolSpec {
                name: "add_entity".into(),
                description: "Record a single entity found in the text.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Entity name as it appears in the text"},
                        "type": type_schema,
                        "description": {"type": "string", "description": "1-2 sentence description"},
                        "attributes": {"type": "object", "description": "Optional key/value attributes"}
                    },
                    "required": ["name", "type"]
                }),
            },
            ToolSpec {
                name: "add_relation".into(),
                description: "Record a relationship between two entities (call add_entity for both first). Direction rule: \"source predicate target\" must read as a TRUE sentence — EVERY predicate ending in _BY (FOUNDED_BY, DEVELOPED_BY, CREATED_BY, INVENTED_BY, PUBLISHED_BY, ...) is passive and points from the thing acted on to the doer (source=Anthropic, predicate=FOUNDED_BY, target=Dario Amodei); a person is never the source of a *_BY relation about their own work. Active predicates point from the doer to the thing. If the sentence is false as written, swap source and target or pick the opposite-voice predicate.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "source": {"type": "string", "description": "Source entity name. For *_BY predicates this is the thing acted on, not the doer."},
                        "predicate": predicate_schema,
                        "target": {"type": "string", "description": "Target entity name. For *_BY predicates this is the doer."},
                        "description": {"type": "string"},
                        "strength": {"type": "number", "description": "Confidence 0..1"}
                    },
                    "required": ["source", "predicate", "target"]
                }),
            },
            ToolSpec {
                name: "add_attribute".into(),
                description: "Attach a key/value attribute to a previously added entity.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "entity": {"type": "string"},
                        "key": {"type": "string"},
                        "value": {"type": "string"}
                    },
                    "required": ["entity", "key", "value"]
                }),
            },
            ToolSpec {
                name: "propose_schema_type".into(),
                description: "Propose a new schema type when the text needs one not in the schema.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "kind": {"type": "string", "enum": ["node", "relation", "attribute"]},
                        "name": {"type": "string"},
                        "reason": {"type": "string"}
                    },
                    "required": ["kind", "name"]
                }),
            },
            ToolSpec {
                name: "list_entities".into(),
                description: "List entities recorded so far (to avoid dangling relations).".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            },
            ToolSpec {
                name: "finish".into(),
                description: "Signal that extraction is complete.".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            },
        ];
        // list_entities only does anything when results are fed back (multi-round).
        tools.retain(|t| t.name != "list_entities" || self.max_rounds > 1);
        tools
    }
}

#[derive(Default)]
struct Accumulator {
    entities: Vec<EntityDraft>,
    attributes: Vec<(String, String, serde_json::Value)>,
    relations: Vec<RelDraft>,
    new_nodes: Vec<String>,
    new_relations: Vec<String>,
    new_attributes: Vec<String>,
    finished: bool,
}

struct EntityDraft {
    name: String,
    type_str: String,
    description: Option<String>,
    attributes: HashMap<String, serde_json::Value>,
}

struct RelDraft {
    source: String,
    predicate: String,
    target: String,
    description: Option<String>,
    strength: f64,
}

fn arg_str(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    })
}

impl ToolCallExtractor {
    fn apply_call(&self, acc: &mut Accumulator, call: &ToolInvocation) -> String {
        match call.name.as_str() {
            "add_entity" => {
                let Some(name) = arg_str(&call.arguments, "name") else {
                    return "error: name required".into();
                };
                let attributes = call
                    .arguments
                    .get("attributes")
                    .and_then(|v| v.as_object())
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default();
                acc.entities.push(EntityDraft {
                    name: name.clone(),
                    type_str: arg_str(&call.arguments, "type").unwrap_or_else(|| "OTHER".into()),
                    description: arg_str(&call.arguments, "description"),
                    attributes,
                });
                format!("ok: entity '{name}' recorded")
            }
            "add_relation" => {
                let (Some(source), Some(predicate), Some(target)) = (
                    arg_str(&call.arguments, "source"),
                    arg_str(&call.arguments, "predicate"),
                    arg_str(&call.arguments, "target"),
                ) else {
                    return "error: source/predicate/target required".into();
                };
                let strength = call
                    .arguments
                    .get("strength")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.8)
                    .clamp(0.0, 1.0);
                acc.relations.push(RelDraft {
                    source,
                    predicate,
                    target,
                    description: arg_str(&call.arguments, "description"),
                    strength,
                });
                "ok: relation recorded".into()
            }
            "add_attribute" => {
                let (Some(entity), Some(key)) = (
                    arg_str(&call.arguments, "entity"),
                    arg_str(&call.arguments, "key"),
                ) else {
                    return "error: entity/key required".into();
                };
                let value = call
                    .arguments
                    .get("value")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                acc.attributes.push((entity, key, value));
                "ok: attribute recorded".into()
            }
            "propose_schema_type" => {
                let kind = arg_str(&call.arguments, "kind").unwrap_or_default();
                let Some(name) = arg_str(&call.arguments, "name") else {
                    return "error: name required".into();
                };
                match kind.as_str() {
                    "node" => acc.new_nodes.push(name),
                    "relation" => acc.new_relations.push(name),
                    "attribute" => acc.new_attributes.push(name),
                    _ => {}
                }
                "ok: schema type proposed".into()
            }
            "list_entities" => {
                let names: Vec<&str> = acc.entities.iter().map(|e| e.name.as_str()).collect();
                serde_json::json!({"entities": names}).to_string()
            }
            "finish" => {
                acc.finished = true;
                "ok: finished".into()
            }
            other => format!("error: unknown tool '{other}'"),
        }
    }

    fn build_graph(&self, acc: &Accumulator) -> KnowledgeGraph {
        // Honour the configured merge strategy on same-name collisions; the
        // post-build `dedup_graph` only matters for the LLM (async) path.
        let strategy = if self.config.spec.merge_duplicates {
            self.config.spec.merge_strategy
        } else {
            MergeStrategy::KeepExisting
        };
        let mut gb = GraphBuilder::new().merge_strategy(strategy);

        // Entities: deduped by lowercased name, combined per the strategy above.
        for draft in &acc.entities {
            gb.add_entity_with_raw_type(
                &draft.name,
                parse_entity_type(&draft.type_str),
                Some(draft.type_str.clone()),
                draft.description.clone(),
                draft.attributes.clone(),
            );
        }

        // Relations: resolve names → ids, drop dangling endpoints, decorate with
        // confidence/description. Before attributes, since add_triple re-inserts
        // its endpoint entities and would otherwise clobber enriched copies.
        for rel in &acc.relations {
            gb.add_relation(
                &rel.source,
                build_predicate(&rel.predicate),
                &rel.target,
                |t| {
                    t.confidence = Some(rel.strength);
                    if let Some(d) = &rel.description {
                        t.metadata
                            .insert("description".into(), serde_json::json!(d));
                    }
                },
            );
        }

        // Standalone attributes last so they survive add_triple re-inserts.
        for (name, key, value) in &acc.attributes {
            gb.set_attribute(name, key.clone(), value.clone());
        }

        gb.into_graph()
    }
}

#[async_trait::async_trait]
impl Extractor for ToolCallExtractor {
    async fn extract(&self, text: &str) -> anyhow::Result<ExtractionResponse> {
        validate_input(text, self.config.min_segment_size, self.quiet)?;
        // Fixed/Evolving constrain to (or evolve from) a seed schema; an empty
        // seed is the degenerate cell of the grid.
        if self.config.spec.mode.needs_schema() && self.config.spec.schema.is_empty() {
            anyhow::bail!(
                "schema mode {:?} requires a non-empty schema (seed one via \
                 ExtractionConfig::from_schema; CLI: --schema <file>), or use SchemaMode::Open",
                self.config.spec.mode
            );
        }
        if !self.backend.supports_tools() {
            anyhow::bail!("the selected backend does not support tool calling");
        }

        let tools = self.tools();
        let opts = CompletionOptions {
            model: self.config.model_name.clone(),
            temperature: 0.3,
            max_tokens: 4000,
        };
        // Schema lives in the system prompt; the user turn carries only the text.
        let user = format!(
            "Extract a knowledge graph from the text below using the tools.\n\nText:\n{text}"
        );
        let mut messages = vec![Message::system(self.system_prompt()), Message::user(user)];

        let mut acc = Accumulator::default();
        for _ in 0..self.max_rounds {
            let resp = self
                .backend
                .complete_with_tools(&messages, &tools, &opts)
                .await?;
            if resp.tool_calls.is_empty() {
                break;
            }

            // Echo the assistant tool calls so multi-round context stays valid.
            let raw_calls: Vec<serde_json::Value> =
                resp.tool_calls.iter().map(|c| c.to_openai_json()).collect();
            messages.push(Message::assistant_with_tool_calls(
                resp.content.clone(),
                raw_calls,
            ));

            for call in &resp.tool_calls {
                let result = self.apply_call(&mut acc, call);
                messages.push(Message::tool_result(call.id.clone(), result));
            }
            if acc.finished {
                break;
            }
        }

        let mut kg = self.build_graph(&acc);
        if self.config.spec.merge_duplicates {
            kg = dedup_graph(kg, self.config.spec.merge_strategy, &self.backend, &opts).await;
        }
        // Tool calls see the whole text at once, so provenance is whole-document.
        {
            let li = crate::citation::LineIndex::new(text);
            let cite =
                crate::citation::Citation::new(self.config.source_doc.clone(), 1, li.total_lines());
            crate::citation::stamp_graph(&mut kg, &cite);
        }
        let mut response = ExtractionResponse::new(kg);
        response.config = Some(self.config.clone());
        response
            .metadata
            .insert("mode".into(), serde_json::json!("toolcall"));
        response.metadata.insert(
            "schema_mode".into(),
            serde_json::json!(self.config.spec.mode.as_str()),
        );
        if !acc.new_nodes.is_empty()
            || !acc.new_relations.is_empty()
            || !acc.new_attributes.is_empty()
        {
            response.metadata.insert(
                "new_schema_types".into(),
                serde_json::json!({
                    "nodes": acc.new_nodes,
                    "relations": acc.new_relations,
                    "attributes": acc.new_attributes,
                }),
            );
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MockBackend;
    use crate::types::PredicateType;

    fn call(name: &str, args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            id: format!("c_{name}"),
            name: name.into(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn single_round_collects_tool_calls() {
        let rounds = vec![vec![
            call(
                "add_entity",
                serde_json::json!({"name": "OpenAI", "type": "ORGANIZATION"}),
            ),
            call(
                "add_entity",
                serde_json::json!({"name": "GPT-4", "type": "TECHNOLOGY"}),
            ),
            call(
                "add_relation",
                serde_json::json!({"source": "OpenAI", "predicate": "DEVELOPED_BY", "target": "GPT-4", "strength": 0.9}),
            ),
            call(
                "add_attribute",
                serde_json::json!({"entity": "GPT-4", "key": "params", "value": "1.8T"}),
            ),
            call("finish", serde_json::json!({})),
        ]];
        let backend = Arc::new(MockBackend::new(vec![]).with_tool_rounds(rounds));
        let ex = ToolCallExtractor::new(backend);
        let out = ex.extract("OpenAI developed GPT-4.").await.unwrap();
        assert_eq!(out.num_entities(), 2);
        assert_eq!(out.num_triples(), 1);
        assert_eq!(
            out.knowledge_graph.triples[0].predicate.predicate_type,
            PredicateType::DevelopedBy
        );
        let gpt = out
            .knowledge_graph
            .entities
            .values()
            .find(|e| e.label == "GPT-4")
            .unwrap();
        assert_eq!(gpt.metadata["params"], serde_json::json!("1.8T"));
    }

    #[tokio::test]
    async fn relation_strength_is_clamped() {
        let rounds = vec![vec![
            call(
                "add_entity",
                serde_json::json!({"name": "A", "type": "OTHER"}),
            ),
            call(
                "add_entity",
                serde_json::json!({"name": "B", "type": "OTHER"}),
            ),
            call(
                "add_relation",
                serde_json::json!({"source": "A", "predicate": "USES", "target": "B", "strength": 5.0}),
            ),
        ]];
        let backend = Arc::new(MockBackend::new(vec![]).with_tool_rounds(rounds));
        let out = ToolCallExtractor::new(backend)
            .extract("text")
            .await
            .unwrap();
        assert_eq!(
            out.knowledge_graph.triples[0].confidence,
            Some(1.0),
            "strength clamps to 1.0"
        );
    }

    #[tokio::test]
    async fn dangling_relation_is_dropped() {
        let rounds = vec![vec![
            call(
                "add_entity",
                serde_json::json!({"name": "OpenAI", "type": "ORGANIZATION"}),
            ),
            call(
                "add_relation",
                serde_json::json!({"source": "OpenAI", "predicate": "USES", "target": "Nonexistent"}),
            ),
        ]];
        let backend = Arc::new(MockBackend::new(vec![]).with_tool_rounds(rounds));
        let out = ToolCallExtractor::new(backend)
            .extract("text")
            .await
            .unwrap();
        assert_eq!(out.num_entities(), 1);
        assert_eq!(out.num_triples(), 0);
    }

    #[tokio::test]
    async fn evolving_mode_records_proposed_types() {
        let rounds = vec![vec![
            call(
                "add_entity",
                serde_json::json!({"name": "Dune", "type": "WORK_OF_ART"}),
            ),
            call(
                "propose_schema_type",
                serde_json::json!({"kind": "node", "name": "Movie"}),
            ),
        ]];
        let backend = Arc::new(MockBackend::new(vec![]).with_tool_rounds(rounds));
        // Evolving requires a non-empty seed schema.
        let cfg = ExtractionConfig::from_schema(Schema::new(
            vec!["WORK_OF_ART".into()],
            vec!["RELATED_TO".into()],
            vec![],
        ));
        let out = ToolCallExtractor::with_config(backend, cfg)
            .schema_mode(SchemaMode::Evolving)
            .extract("text")
            .await
            .unwrap();
        assert_eq!(
            out.metadata["new_schema_types"]["nodes"][0],
            serde_json::json!("Movie")
        );
        assert_eq!(out.metadata["schema_mode"], serde_json::json!("evolving"));
    }

    #[tokio::test]
    async fn fixed_mode_without_schema_errors() {
        // Fixed on an empty schema is the degenerate combo — must error.
        let backend = Arc::new(MockBackend::new(vec![]).with_tool_rounds(vec![vec![]]));
        let err = ToolCallExtractor::new(backend)
            .schema_mode(SchemaMode::Fixed)
            .extract("text")
            .await;
        assert!(err.is_err(), "Fixed mode with an empty schema must error");
    }
}
