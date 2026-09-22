//! Conversion to shared `core-types-rs` knowledge graph protocol types.

use std::collections::{BTreeMap, HashMap};

use core_types_rs::{
    KgDocument, KgEntity, KgEvidence, KgRelation, LineSpan, SourceRange, KG_PROTOCOL_VERSION,
};
use serde_json::Value;

use crate::citation::CITATIONS_KEY;
use crate::types::{KnowledgeGraph, Triple};

impl KnowledgeGraph {
    /// Convert this extractor-domain graph into the portable KG protocol shape.
    pub fn to_kg_document(&self) -> KgDocument {
        KgDocument {
            schema_version: KG_PROTOCOL_VERSION.to_string(),
            entities: self
                .entities
                .iter()
                .map(|(_, entity)| KgEntity {
                    id: entity.id.clone(),
                    label: entity.label.clone(),
                    entity_type: entity.output_type(),
                    description: entity.description.clone(),
                    confidence: entity.confidence,
                    properties: entity_properties(entity),
                    evidence: citations_to_evidence(&entity.metadata),
                })
                .collect(),
            relations: self
                .triples
                .iter()
                .map(|triple| KgRelation {
                    id: None,
                    subject: triple.subject.id.clone(),
                    predicate: triple.predicate.output_type(),
                    object: triple.object.id.clone(),
                    label: triple.predicate.label.clone(),
                    confidence: triple.confidence.or(triple.predicate.confidence),
                    properties: relation_properties(triple),
                    evidence: citations_to_evidence(&triple.metadata),
                })
                .collect(),
            hyperedges: Vec::new(),
            schema: None,
            metadata: metadata_to_properties(&self.metadata),
        }
    }
}

fn entity_properties(entity: &crate::types::Entity) -> BTreeMap<String, Value> {
    let mut properties = record_metadata_to_properties(&entity.metadata);
    properties.insert(
        "normalized_entity_type".into(),
        Value::String(entity.entity_type.value()),
    );
    properties
}

fn relation_properties(triple: &Triple) -> BTreeMap<String, Value> {
    let mut properties = record_metadata_to_properties(&triple.metadata);
    properties.insert(
        "normalized_predicate_type".into(),
        Value::String(triple.predicate.predicate_type.value()),
    );
    if !triple.predicate.metadata.is_empty() {
        properties.insert(
            "predicate_metadata".into(),
            Value::Object(
                triple
                    .predicate
                    .metadata
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
        );
    }
    properties
}

fn record_metadata_to_properties(metadata: &HashMap<String, Value>) -> BTreeMap<String, Value> {
    let citations_promoted =
        parse_internal_citations(metadata).is_some_and(|evidence| !evidence.is_empty());
    metadata
        .iter()
        .filter(|(key, _)| !citations_promoted || key.as_str() != CITATIONS_KEY)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn metadata_to_properties(metadata: &HashMap<String, Value>) -> BTreeMap<String, Value> {
    metadata
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn citations_to_evidence(metadata: &HashMap<String, Value>) -> Vec<KgEvidence> {
    parse_internal_citations(metadata).unwrap_or_default()
}

fn parse_internal_citations(metadata: &HashMap<String, Value>) -> Option<Vec<KgEvidence>> {
    let Some(Value::Array(citations)) = metadata.get(CITATIONS_KEY) else {
        return None;
    };

    citations
        .iter()
        .map(internal_citation_to_evidence)
        .collect()
}

fn internal_citation_to_evidence(citation: &Value) -> Option<KgEvidence> {
    let object = citation.as_object()?;
    let source_file = match object.get("doc")? {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        _ => return None,
    };
    let range = if object.len() == 2 && object.contains_key("range") {
        object
            .get("range")
            .and_then(source_range_from_value)
            .filter(source_range_has_coordinates)?
    } else if object.len() == 2 && object.contains_key("lines") {
        object.get("lines").and_then(lines_to_source_range)?
    } else {
        return None;
    };
    Some(KgEvidence {
        source_file,
        source_id: None,
        range: Some(range),
        quote: None,
        metadata: BTreeMap::new(),
    })
}

fn source_range_has_coordinates(range: &SourceRange) -> bool {
    range.char_span.is_some()
        || range.line.is_some()
        || range.page.is_some()
        || range.bbox.is_some()
}

fn source_range_from_value(value: &Value) -> Option<SourceRange> {
    let object = value.as_object()?;
    if object.is_empty()
        || object
            .keys()
            .any(|key| !["char_span", "line", "page", "bbox"].contains(&key.as_str()))
    {
        return None;
    }
    for key in ["char_span", "line", "page"] {
        if object
            .get(key)
            .is_some_and(|span| !object_has_exact_keys(span, &["start", "end"]))
        {
            return None;
        }
    }
    if object
        .get("bbox")
        .is_some_and(|bbox| !object_has_exact_keys(bbox, &["x0", "y0", "x1", "y1"]))
    {
        return None;
    }
    serde_json::from_value(value.clone()).ok()
}

fn object_has_exact_keys(value: &Value, expected: &[&str]) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

fn lines_to_source_range(value: &Value) -> Option<SourceRange> {
    let lines = value.as_array()?;
    if lines.len() != 2 {
        return None;
    }
    let start = lines.first()?.as_u64()?;
    let end = lines.get(1)?.as_u64()?;
    let start = u32::try_from(start).ok()?;
    let end = u32::try_from(end).ok()?;
    let line = LineSpan::new(start, end)?;
    Some(SourceRange {
        char_span: None,
        line: Some(line),
        ..SourceRange::default()
    })
}

/// Keys [`to_kg_document`](KnowledgeGraph::to_kg_document) *derives* on export;
/// on import they are re-derived from the resolved types, so the incoming
/// values are dropped instead of round-tripped as user metadata.
const DERIVED_ENTITY_PROPERTY: &str = "normalized_entity_type";
const DERIVED_RELATION_PROPERTY: &str = "normalized_predicate_type";
const DERIVED_PREDICATE_PROPERTY: &str = "predicate_metadata";

/// Report of what [`KnowledgeGraph::from_kg_document`] could not carry over,
/// surfaced as invoke diagnostics by the provider surface.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// Relations dropped because an endpoint id is not a declared entity
    /// (same dangling-drop rule as the extraction-time GraphBuilder).
    pub dangling_relations: usize,
    /// Evidence entries dropped because they carry no `range` (only ranged
    /// evidence round-trips through the internal citation shape).
    pub range_less_evidence: usize,
}

impl KnowledgeGraph {
    /// Import a portable `kg.protocol.v1` document back into the extractor-domain
    /// graph — the inverse of [`to_kg_document`](Self::to_kg_document), used by
    /// the provider surface's graph-in/graph-out capabilities
    /// (`detect.communities*`, `resolve.*`).
    ///
    /// Type tokens are re-resolved through `EntityType::resolve` /
    /// `PredicateType::resolve` (exact → alias → OTHER fallback, same as the
    /// engines) and the original token is kept as `raw_type`, so a
    /// `from ∘ to` round trip re-emits the document's own tokens. Properties
    /// become record metadata minus the derived `normalized_*` keys; ranged
    /// evidence becomes internal citations (re-promoted on the next export).
    /// Relations with dangling endpoints are dropped, matching the
    /// extraction-time rule; drops are counted in the returned
    /// [`ImportReport`], never silently ignored.
    pub fn from_kg_document(doc: &KgDocument) -> (KnowledgeGraph, ImportReport) {
        let mut report = ImportReport::default();
        let mut kg = KnowledgeGraph::new();
        kg.metadata = doc
            .metadata
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for e in &doc.entities {
            let (entity_type, _) = crate::types::EntityType::resolve(&e.entity_type);
            let mut entity = crate::types::Entity::new(e.id.clone(), e.label.clone(), entity_type)
                .with_raw_type(e.entity_type.clone());
            entity.confidence = e.confidence;
            entity.description = e.description.clone();
            entity.metadata = import_properties(
                &e.properties,
                &[DERIVED_ENTITY_PROPERTY],
                &e.evidence,
                &mut report,
            );
            kg.add_entity(entity);
        }
        for r in &doc.relations {
            let (Some(subject), Some(object)) =
                (kg.get_entity(&r.subject), kg.get_entity(&r.object))
            else {
                report.dangling_relations += 1;
                continue;
            };
            let (subject, object) = (subject.clone(), object.clone());
            let (predicate_type, _) = crate::types::PredicateType::resolve(&r.predicate);
            let mut predicate = crate::types::Predicate::with_label(
                predicate_type,
                r.label.clone().unwrap_or_else(|| r.predicate.clone()),
            );
            predicate.raw_type = Some(r.predicate.clone());
            if let Some(Value::Object(map)) = r.properties.get(DERIVED_PREDICATE_PROPERTY) {
                predicate.metadata = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            }
            let mut triple = Triple::new(subject, predicate, object);
            triple.confidence = r.confidence;
            triple.metadata = import_properties(
                &r.properties,
                &[DERIVED_RELATION_PROPERTY, DERIVED_PREDICATE_PROPERTY],
                &r.evidence,
                &mut report,
            );
            kg.add_triple(triple);
        }
        (kg, report)
    }
}

/// Properties → record metadata: derived keys skipped, ranged evidence folded
/// in as internal citations (deduplicated by [`attach_citation`]).
fn import_properties(
    properties: &BTreeMap<String, Value>,
    derived: &[&str],
    evidence: &[KgEvidence],
    report: &mut ImportReport,
) -> HashMap<String, Value> {
    let mut metadata: HashMap<String, Value> = properties
        .iter()
        .filter(|(k, _)| !derived.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for ev in evidence {
        match &ev.range {
            Some(range) => crate::citation::attach_citation(
                &mut metadata,
                &crate::citation::Citation::from_range(ev.source_file.clone(), range.clone()),
            ),
            None => report.range_less_evidence += 1,
        }
    }
    metadata
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;

