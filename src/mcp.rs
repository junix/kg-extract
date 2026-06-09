//! Persistent knowledge-graph store backing `kg-extract-mcp`.
//!
//! A deterministic graph accumulator: each operation loads the graph at
//! `<output>/<path>.json`, merges a single delta (entity / relation /
//! attribute), and writes it back. **No LLM is involved** — the *caller* (an MCP
//! client) does the reasoning and drives these function-call style mutations,
//! mirroring the tool set of [`ToolCallExtractor`](crate::extractor::ToolCallExtractor).
//!
//! Entity identity is `entity_<md5(name)[..8]>` — the same scheme the extractors
//! use, so files produced by the CLI and by the MCP server are interchangeable.
//! Dedup on merge is by entity id (confidence wins) and by triple
//! `(subject_id, predicate_type, object_id)`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::types::{Entity, EntityType, KnowledgeGraph, Predicate, PredicateType, Triple};

/// Stable id for an entity name (matches `simple.rs` / `toolcall.rs`).
pub fn entity_id(name: &str) -> String {
    let digest = format!("{:x}", md5::compute(name.as_bytes()));
    format!("entity_{}", &digest[..8])
}

fn parse_entity_type(s: &str) -> EntityType {
    let s = s.trim();
    if s.is_empty() {
        return EntityType::from_loose("other");
    }
    s.parse::<EntityType>().unwrap_or_else(|_| EntityType::from_loose(s))
}

fn build_predicate(s: &str) -> Predicate {
    let pt = s
        .to_uppercase()
        .replace([' ', '-'], "_")
        .parse::<PredicateType>()
        .unwrap_or_else(|_| PredicateType::from_loose(s));
    Predicate::with_label(pt, s.to_string())
}

/// A directory-backed collection of knowledge graphs, one JSON file per `path`.
///
/// Each mutating call is a load-modify-save cycle serialized by `lock`, so
/// concurrent tool calls (rmcp dispatches requests concurrently) can't clobber
/// each other's writes to the same graph file.
#[derive(Debug)]
pub struct KgStore {
    output: PathBuf,
    lock: std::sync::Mutex<()>,
}

impl KgStore {
    pub fn new(output: impl Into<PathBuf>) -> Self {
        Self { output: output.into(), lock: std::sync::Mutex::new(()) }
    }

    pub fn output_dir(&self) -> &Path {
        &self.output
    }

    /// Map a caller-supplied `path` to `<output>/<path>.json`, rejecting absolute
    /// paths and any `..` component so results can never escape `<output>`.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let rel = rel.trim();
        if rel.is_empty() {
            anyhow::bail!("path must not be empty");
        }
        let stem = rel.strip_suffix(".json").unwrap_or(rel);
        let mut out = self.output.clone();
        let mut pushed = 0usize;
        for comp in stem.split(['/', '\\']) {
            match comp {
                "" | "." => continue,
                ".." => anyhow::bail!("path must not contain '..': {rel}"),
                _ => {
                    out.push(comp);
                    pushed += 1;
                }
            }
        }
        if pushed == 0 {
            anyhow::bail!("path resolves to the output directory itself: {rel}");
        }
        out.set_extension("json");
        Ok(out)
    }

    /// Load the graph at `path`, or an empty graph if the file does not exist.
    pub fn load(&self, rel: &str) -> Result<KnowledgeGraph> {
        let p = self.resolve(rel)?;
        if !p.exists() {
            return Ok(KnowledgeGraph::new());
        }
        let body = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        if body.trim().is_empty() {
            return Ok(KnowledgeGraph::new());
        }
        serde_json::from_str(&body).with_context(|| format!("parsing {}", p.display()))
    }

    /// Write the graph to `path` (creating parent directories), returning the
    /// absolute file path written.
    pub fn save(&self, rel: &str, kg: &KnowledgeGraph) -> Result<PathBuf> {
        let p = self.resolve(rel)?;
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(kg)?;
        std::fs::write(&p, body).with_context(|| format!("writing {}", p.display()))?;
        Ok(p)
    }

    /// Record an entity, merging into any existing entity with the same name.
    pub fn add_entity(
        &self,
        rel: &str,
        name: &str,
        type_str: &str,
        description: Option<String>,
        attributes: HashMap<String, Value>,
    ) -> Result<Value> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("name must not be empty");
        }
        let _guard = self.lock.lock().unwrap();
        let mut kg = self.load(rel)?;
        let id = entity_id(name);
        let etype = parse_entity_type(type_str);
        if let Some(existing) = kg.entities.get_mut(&id) {
            existing.entity_type = etype;
            if let Some(d) = description {
                existing.description = Some(d);
            }
            for (k, v) in attributes {
                existing.metadata.insert(k, v);
            }
        } else {
            let mut e = Entity::new(id, name, etype);
            e.description = description;
            e.metadata = attributes;
            kg.add_entity(e);
        }
        let path = self.save(rel, &kg)?;
        Ok(report(&path, &kg, format!("entity '{name}' stored")))
    }

    /// Record a relationship; endpoints are auto-created as stub entities if
    /// absent, so a relation is never silently dropped. Deduplicated by
    /// `(subject, predicate, object)`.
    pub fn add_relation(
        &self,
        rel_path: &str,
        source: &str,
        predicate: &str,
        target: &str,
        description: Option<String>,
        strength: Option<f64>,
    ) -> Result<Value> {
        let (source, target, predicate) = (source.trim(), target.trim(), predicate.trim());
        if source.is_empty() || target.is_empty() || predicate.is_empty() {
            anyhow::bail!("source, predicate and target are required");
        }
        let _guard = self.lock.lock().unwrap();
        let mut kg = self.load(rel_path)?;
        let sid = entity_id(source);
        let tid = entity_id(target);
        // Strict: both endpoints must already exist. The error names the missing
        // endpoint(s), tells the caller exactly what to do, and lists the known
        // entity labels so it can fix a typo / name variant on the next round.
        let mut missing: Vec<&str> = Vec::new();
        if !kg.entities.contains_key(&sid) {
            missing.push(source);
        }
        if !kg.entities.contains_key(&tid) && target != source {
            missing.push(target);
        }
        if !missing.is_empty() {
            let which =
                missing.iter().map(|m| format!("'{m}'")).collect::<Vec<_>>().join(" and ");
            anyhow::bail!(
                "entity {which} not found in graph '{rel_path}'. Call add_entity for {which} \
                 first (same path='{rel_path}'), then retry add_relation. Known entities: {}",
                known_labels(&kg)
            );
        }
        let subject = kg.entities.get(&sid).expect("checked present").clone();
        let object = kg.entities.get(&tid).expect("checked present").clone();

        let mut triple = Triple::new(subject, build_predicate(predicate), object);
        triple.confidence = Some(strength.unwrap_or(0.8));
        if let Some(d) = description {
            triple.metadata.insert("description".into(), serde_json::json!(d));
        }
        let key = triple.to_tuple();
        let added = !kg.triples.iter().any(|t| t.to_tuple() == key);
        if added {
            kg.add_triple(triple);
        }
        let path = self.save(rel_path, &kg)?;
        let msg = if added {
            format!("relation '{source}' -{predicate}-> '{target}' stored")
        } else {
            format!("relation '{source}' -{predicate}-> '{target}' already present")
        };
        Ok(report(&path, &kg, msg))
    }

    /// Attach a key/value attribute to a previously stored entity.
    pub fn add_attribute(&self, rel: &str, entity: &str, key: &str, value: Value) -> Result<Value> {
        let (entity, key) = (entity.trim(), key.trim());
        if entity.is_empty() || key.is_empty() {
            anyhow::bail!("entity and key are required");
        }
        let _guard = self.lock.lock().unwrap();
        let mut kg = self.load(rel)?;
        let id = entity_id(entity);
        match kg.entities.get_mut(&id) {
            Some(e) => {
                e.metadata.insert(key.to_string(), value);
            }
            None => anyhow::bail!(
                "entity '{entity}' not found in graph '{rel}'. Call add_entity(path='{rel}', \
                 name='{entity}', …) first, then retry add_attribute. Known entities: {}",
                known_labels(&kg)
            ),
        }
        let path = self.save(rel, &kg)?;
        Ok(report(&path, &kg, format!("attribute '{key}' set on '{entity}'")))
    }

    /// Query the graph at `path`. `view` selects what to return:
    /// - `summary` (default): stats only — cheapest, protects caller context.
    /// - `entities`: `{id,label,type}` list (capped by `limit`).
    /// - `relations`: `{source,predicate,target}` list (capped by `limit`).
    /// - `neighbors`: focal `entity` (full detail) + its incoming/outgoing edges.
    /// - `full`: both entities and relations (capped).
    ///
    /// Errors (actionably) on a missing graph, an unknown `view`, or a
    /// `neighbors` query whose `entity` is absent.
    pub fn query_graph(
        &self,
        rel: &str,
        view: &str,
        entity: Option<&str>,
        limit: usize,
    ) -> Result<Value> {
        let _guard = self.lock.lock().unwrap();
        let path = self.resolve(rel)?;
        if !path.exists() {
            anyhow::bail!(
                "no graph found at path '{rel}' ({}). Nothing has been stored there yet — \
                 call add_entity(path='{rel}', …) to create it.",
                path.display()
            );
        }
        let kg = self.load(rel)?;
        let mut out = serde_json::json!({
            "path": path.display().to_string(),
            "view": view,
            "stats": kg.stats(),
        });
        let obj = out.as_object_mut().unwrap();
        match view {
            "summary" => {}
            "entities" => {
                let (items, truncated) = take_entities(&kg, limit);
                obj.insert("entities".into(), Value::Array(items));
                if truncated {
                    obj.insert("truncated".into(), Value::Bool(true));
                }
            }
            "relations" => {
                let (items, truncated) = take_relations(&kg, limit);
                obj.insert("relations".into(), Value::Array(items));
                if truncated {
                    obj.insert("truncated".into(), Value::Bool(true));
                }
            }
            "full" => {
                let (e, te) = take_entities(&kg, limit);
                let (r, tr) = take_relations(&kg, limit);
                obj.insert("entities".into(), Value::Array(e));
                obj.insert("relations".into(), Value::Array(r));
                if te || tr {
                    obj.insert("truncated".into(), Value::Bool(true));
                }
            }
            "neighbors" => {
                let name = entity
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("view='neighbors' requires the `entity` argument (an entity name)")
                    })?;
                let id = entity_id(name);
                let focal = kg.entities.get(&id).ok_or_else(|| {
                    anyhow::anyhow!(
                        "entity '{name}' not found in graph '{rel}'. Known entities: {}",
                        known_labels(&kg)
                    )
                })?;
                let mut outgoing = Vec::new();
                let mut incoming = Vec::new();
                for t in &kg.triples {
                    if t.subject.id == id {
                        outgoing.push(serde_json::json!({
                            "predicate": t.predicate.display_label(),
                            "target": t.object.label,
                            "target_type": t.object.entity_type.value(),
                        }));
                    } else if t.object.id == id {
                        incoming.push(serde_json::json!({
                            "predicate": t.predicate.display_label(),
                            "source": t.subject.label,
                            "source_type": t.subject.entity_type.value(),
                        }));
                    }
                }
                obj.insert(
                    "entity".into(),
                    serde_json::json!({
                        "id": focal.id,
                        "label": focal.label,
                        "type": focal.entity_type.value(),
                        "description": focal.description,
                        "attributes": focal.metadata,
                    }),
                );
                obj.insert("outgoing".into(), Value::Array(outgoing));
                obj.insert("incoming".into(), Value::Array(incoming));
            }
            other => anyhow::bail!(
                "unknown view '{other}'. Valid views: summary, entities, relations, neighbors, full."
            ),
        }
        Ok(out)
    }
}

/// Up to `limit` entities as `{id,label,type}`; bool = whether more were dropped.
fn take_entities(kg: &KnowledgeGraph, limit: usize) -> (Vec<Value>, bool) {
    let items = kg
        .entities
        .iter()
        .take(limit)
        .map(|(id, e)| serde_json::json!({ "id": id, "label": e.label, "type": e.entity_type.value() }))
        .collect();
    (items, kg.entities.len() > limit)
}

/// Up to `limit` relations as `{source,predicate,target}`; bool = more dropped.
fn take_relations(kg: &KnowledgeGraph, limit: usize) -> (Vec<Value>, bool) {
    let items = kg
        .triples
        .iter()
        .take(limit)
        .map(|t| {
            serde_json::json!({
                "source": t.subject.label,
                "predicate": t.predicate.display_label(),
                "target": t.object.label,
            })
        })
        .collect();
    (items, kg.triples.len() > limit)
}

/// Comma-joined entity labels (capped) for actionable "not found" errors, so the
/// caller can spot a typo / name variant and retry with the right surface form.
fn known_labels(kg: &KnowledgeGraph) -> String {
    const MAX: usize = 40;
    let labels: Vec<&str> = kg.entities.iter().map(|(_, e)| e.label.as_str()).collect();
    if labels.is_empty() {
        return "(none yet)".into();
    }
    let mut s = labels.iter().take(MAX).copied().collect::<Vec<_>>().join(", ");
    if labels.len() > MAX {
        s.push_str(&format!(", … (+{} more)", labels.len() - MAX));
    }
    s
}

fn report(path: &Path, kg: &KnowledgeGraph, message: String) -> Value {
    serde_json::json!({
        "ok": true,
        "message": message,
        "path": path.display().to_string(),
        "stats": kg.stats(),
    })
}

#[cfg(test)]
#[path = "mcp_test.rs"]
mod tests;
