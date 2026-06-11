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
                    entity_type: entity.entity_type.value(),
                    description: entity.description.clone(),
                    confidence: entity.confidence,
                    properties: metadata_to_properties(&entity.metadata),
                    evidence: citations_to_evidence(&entity.metadata),
                })
                .collect(),
            relations: self
                .triples
                .iter()
                .map(|triple| KgRelation {
                    id: None,
                    subject: triple.subject.id.clone(),
                    predicate: triple.predicate.predicate_type.value(),
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

fn relation_properties(triple: &Triple) -> BTreeMap<String, Value> {
    let mut properties = metadata_to_properties(&triple.metadata);
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

fn metadata_to_properties(metadata: &HashMap<String, Value>) -> BTreeMap<String, Value> {
    metadata
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn citations_to_evidence(metadata: &HashMap<String, Value>) -> Vec<KgEvidence> {
    let Some(Value::Array(citations)) = metadata.get(CITATIONS_KEY) else {
        return Vec::new();
    };

    citations
        .iter()
        .filter_map(|citation| {
            let object = citation.as_object()?;
            let source_file = object
                .get("doc")
                .and_then(Value::as_str)
                .map(str::to_string);
            let range = object.get("lines").and_then(lines_to_source_range);
            Some(KgEvidence {
                source_file,
                source_id: None,
                range,
                quote: None,
                metadata: BTreeMap::new(),
            })
        })
        .collect()
}

fn lines_to_source_range(value: &Value) -> Option<SourceRange> {
    let lines = value.as_array()?;
    let start = lines.first()?.as_u64()?;
    let end = lines.get(1)?.as_u64()?;
    let start = u32::try_from(start).ok()?;
    let end = u32::try_from(end).ok()?;
    Some(SourceRange {
        char_span: None,
        line: LineSpan::new(start, end),
        page: None,
    })
}

#[cfg(test)]
mod tests {
    use core_types_rs::KG_PROTOCOL_VERSION;
    use serde_json::json;

    use crate::citation::{attach_citation, Citation};
    use crate::types::{Entity, EntityType, KnowledgeGraph, Predicate, PredicateType, Triple};

    #[test]
    fn knowledge_graph_converts_to_portable_kg_protocol() {
        let mut openai = Entity::new("entity_openai", "OpenAI", EntityType::Organization);
        openai.confidence = Some(0.9);
        openai.metadata.insert("alias".into(), json!("Open AI"));
        attach_citation(
            &mut openai.metadata,
            &Citation::new(Some("doc.md".into()), 3, 5),
        );
        let gpt4 = Entity::new("entity_gpt4", "GPT-4", EntityType::Technology);

        let mut predicate = Predicate::with_label(PredicateType::DevelopedBy, "developed");
        predicate
            .metadata
            .insert("source".into(), json!("schema-json"));
        let mut triple = Triple::new(openai.clone(), predicate, gpt4.clone());
        triple.confidence = Some(0.8);

        let mut kg = KnowledgeGraph::new();
        kg.add_entity(openai);
        kg.add_entity(gpt4);
        kg.add_triple(triple);

        let doc = kg.to_kg_document();
        assert_eq!(doc.schema_version, KG_PROTOCOL_VERSION);
        assert_eq!(doc.entities.len(), 2);
        assert_eq!(doc.entities[0].entity_type, "ORGANIZATION");
        assert_eq!(
            doc.entities[0].evidence[0].source_file.as_deref(),
            Some("doc.md")
        );
        assert_eq!(
            doc.entities[0].evidence[0]
                .range
                .as_ref()
                .unwrap()
                .line
                .unwrap()
                .start,
            3
        );
        assert_eq!(doc.relations.len(), 1);
        assert_eq!(doc.relations[0].subject, "entity_openai");
        assert_eq!(doc.relations[0].predicate, "DEVELOPED_BY");
        assert_eq!(doc.relations[0].object, "entity_gpt4");
        assert!(doc.relations[0]
            .properties
            .contains_key("predicate_metadata"));
    }
}
