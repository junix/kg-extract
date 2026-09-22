//! SchemaJsonExtractor — schema-driven extraction with three schema modes:
//! `Open` (no predefined types), `Fixed` (closed schema), and `Evolving`
//! (seed schema the model may extend). Ported from `graph/kg_extractor/youtu.py`.

use super::{validate_input, Extractor, SchemaMode};
use crate::backend::{CompletionOptions, LlmBackend, Message};
use crate::graph_build::GraphBuilder;
use crate::json::extract_json_from_response;
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
        // Canonical direction normalisation (opt-in) runs before dedup so
        // direction variants (`USES` / `IS_USED_BY`) share one dedup key.
        if self.config.spec.canonical_direction {
            crate::merger::normalize_direction(&mut kg);
        }
        // Dedup pass, mirroring ToolCall. `GraphBuilder` dedups *entities* by
        // lowercased name but `KnowledgeGraph::add_triple` is a bare push, so
        // without this a model that emits the same relation twice yields two
        // identical triples — and `spec.coref` had no reader on this engine at
        // all, making `--coref` a silent no-op here.
        if self.config.spec.merge_duplicates {
            kg = crate::merger::dedup_graph_coref(
                kg,
                self.config.spec.merge_strategy,
                self.config.spec.coref,
                &self.backend,
                &opts,
            )
            .await;
        }
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
#[path = "schema_json_tests.rs"]
mod tests;
