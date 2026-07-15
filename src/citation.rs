//! Provenance support: which document and line range each entity / triple
//! came from.
//!
//! Citation stamping is part of the default extraction path.
//!
//! Citations are stored in the open `metadata` map of [`Entity`] / [`Triple`]
//! under [`CITATIONS_KEY`]. Existing line-only citations keep the legacy
//! `{"doc": <name|null>, "lines": [start, end]}` shape; citations carrying
//! richer protocol coordinates use `{"doc": ..., "range": SourceRange}`.
//! Readers accept both shapes. A record seen in several places carries several
//! citations; merging unions them.
//!
//! Line ranges are computed **by our code** from the chunk/slice char offsets
//! the chunker already tracks — the model is never asked to count lines, so
//! the ranges cannot be hallucinated. Granularity is therefore the chunk (or
//! slice) that contained the mention, not the mention itself.

use std::collections::HashMap;

use core_types_rs::{LineSpan, SourceRange};
use serde_json::{json, Value};

use crate::types::KnowledgeGraph;

/// Metadata key under which citation arrays live.
pub const CITATIONS_KEY: &str = "citations";

/// One provenance record: a document name (when known) plus its complete
/// shared-protocol source range.
#[derive(Debug, Clone, PartialEq)]
pub struct Citation {
    pub doc: Option<String>,
    pub range: SourceRange,
}

impl Citation {
    /// Construct a legacy-compatible line-only citation.
    pub fn new(doc: Option<String>, start_line: usize, end_line: usize) -> Self {
        let line = u32::try_from(start_line)
            .ok()
            .zip(u32::try_from(end_line).ok())
            .and_then(|(start, end)| LineSpan::new(start, end));
        Self {
            doc,
            range: SourceRange {
                line,
                ..SourceRange::default()
            },
        }
    }

    pub fn from_range(doc: Option<String>, range: SourceRange) -> Self {
        Self { doc, range }
    }

    pub fn to_value(&self) -> Value {
        // Preserve the public metadata shape used before SourceRange gained
        // spatial provenance. A page or bbox requires the richer shape; line-
        // only citations remain byte-shape compatible with existing files.
        if self.range.page.is_none() && self.range.bbox.is_none() {
            if let Some(line) = self.range.line {
                return json!({ "doc": self.doc, "lines": [line.start, line.end] });
            }
        }
        json!({ "doc": self.doc, "range": &self.range })
    }
}

/// Char-offset → line-number lookup for one document.
///
/// Offsets are **char** indices (what chonkie's `range.char_span`
/// carries), not bytes, so multibyte text maps correctly.
#[derive(Debug)]
pub struct LineIndex {
    /// Char offset at which each line starts; `line_starts[0] == 0`.
    line_starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        for (i, ch) in text.chars().enumerate() {
            if ch == '\n' {
                line_starts.push(i + 1);
            }
        }
        LineIndex { line_starts }
    }

    pub fn total_lines(&self) -> usize {
        self.line_starts.len()
    }

    /// 1-based line containing the char at `offset` (clamped to the last line).
    pub fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i + 1,
            Err(i) => i, // insertion point i ⇒ line_starts[i-1] <= offset
        }
    }

    /// Inclusive 1-based line range for the chars `[start, end)` (the chunker's
    /// half-open convention). An empty range cites the line of `start`.
    pub fn line_range(&self, start: usize, end: usize) -> (usize, usize) {
        let last_char = end.saturating_sub(1).max(start);
        (self.line_of(start), self.line_of(last_char))
    }
}

/// Append `citation` to `metadata[CITATIONS_KEY]`, skipping exact duplicates.
pub fn attach_citation(metadata: &mut HashMap<String, Value>, citation: &Citation) {
    let entry = metadata
        .entry(CITATIONS_KEY.to_string())
        .or_insert_with(|| Value::Array(vec![]));
    let Value::Array(list) = entry else {
        return; // foreign value under our key — leave it alone
    };
    let v = citation.to_value();
    if !list.contains(&v) {
        list.push(v);
    }
}

/// Union `src`'s citations into `dst` (deduplicated). Used when two records
/// judged identical are merged, so provenance from both sides survives.
pub fn union_citations(dst: &mut HashMap<String, Value>, src: &HashMap<String, Value>) {
    let Some(Value::Array(src_list)) = src.get(CITATIONS_KEY) else {
        return;
    };
    if src_list.is_empty() {
        return;
    }
    let entry = dst
        .entry(CITATIONS_KEY.to_string())
        .or_insert_with(|| Value::Array(vec![]));
    let Value::Array(dst_list) = entry else {
        return;
    };
    for v in src_list {
        if !dst_list.contains(v) {
            dst_list.push(v.clone());
        }
    }
}

/// Stamp every entity and triple of `kg` with `citation`. Triple endpoint
/// snapshots are stamped too: `KnowledgeGraph::add_triple` re-inserts them
/// into the entity table, so an unstamped snapshot would erase the entity's
/// provenance downstream.
pub fn stamp_graph(kg: &mut KnowledgeGraph, citation: &Citation) {
    for e in kg.entities.values_mut() {
        attach_citation(&mut e.metadata, citation);
    }
    for t in kg.triples.iter_mut() {
        attach_citation(&mut t.metadata, citation);
        attach_citation(&mut t.subject.metadata, citation);
        attach_citation(&mut t.object.metadata, citation);
    }
}

/// Stamp whole-document provenance onto every entity and triple of `kg`: one
/// citation spanning line 1 through the last line of `text`. This is the
/// convention used by single-shot engines (SchemaJson, ToolCall) that feed the
/// entire document through in one pass. Multi-slice engines (e.g. Agentic)
/// cite per-slice instead and must not use this — `text`'s line count would
/// not match any individual slice.
pub fn stamp_whole_document(kg: &mut KnowledgeGraph, source_doc: &Option<String>, text: &str) {
    let li = LineIndex::new(text);
    let cite = Citation::new(source_doc.clone(), 1, li.total_lines());
    stamp_graph(kg, &cite);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_index_maps_multibyte_text_by_chars() {
        // 3 lines of Chinese; offsets are chars, so line 2 starts at char 6
        // (5 chars + '\n'), NOT at a byte boundary.
        let text = "第一行内容\n第二行内容\n第三行";
        let li = LineIndex::new(text);
        assert_eq!(li.total_lines(), 3);
        assert_eq!(li.line_of(0), 1);
        assert_eq!(li.line_of(5), 1); // the '\n' belongs to line 1
        assert_eq!(li.line_of(6), 2);
        assert_eq!(li.line_of(12), 3);
        assert_eq!(li.line_range(0, 11), (1, 2)); // chars 0..11 span lines 1-2
        assert_eq!(li.line_range(12, 15), (3, 3));
    }

    #[test]
    fn line_range_of_empty_slice_cites_its_start_line() {
        let li = LineIndex::new("a\nb\nc");
        assert_eq!(li.line_range(2, 2), (2, 2));
    }

    #[test]
    fn attach_deduplicates_identical_citations() {
        let mut meta = HashMap::new();
        let c = Citation::new(Some("doc.md".into()), 3, 7);
        attach_citation(&mut meta, &c);
        attach_citation(&mut meta, &c);
        let list = meta.get(CITATIONS_KEY).and_then(|v| v.as_array()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], json!({"doc": "doc.md", "lines": [3, 7]}));
    }

    #[test]
    fn union_merges_distinct_and_skips_duplicate_citations() {
        let mut a = HashMap::new();
        let mut b = HashMap::new();
        attach_citation(&mut a, &Citation::new(Some("x.md".into()), 1, 4));
        attach_citation(&mut b, &Citation::new(Some("x.md".into()), 1, 4)); // dup
        attach_citation(&mut b, &Citation::new(Some("y.md".into()), 9, 12)); // new
        union_citations(&mut a, &b);
        let list = a.get(CITATIONS_KEY).and_then(|v| v.as_array()).unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn add_triple_endpoint_overwrite_keeps_entity_citations() {
        use crate::types::{Entity, EntityType, Predicate, PredicateType, Triple};

        // Regression: add_triple re-inserts endpoint snapshots into the entity
        // table; a snapshot without citations must not erase the provenance
        // already recorded on the stored entity.
        let mut kg = KnowledgeGraph::new();
        let mut a = Entity::new("e1", "Alpha", EntityType::Organization);
        attach_citation(&mut a.metadata, &Citation::new(Some("doc.md".into()), 1, 4));
        kg.add_entity(a.clone());

        let bare_a = Entity::new("e1", "Alpha", EntityType::Organization); // no citations
        let b = Entity::new("e2", "Beta", EntityType::Organization);
        let t = Triple::new(bare_a, Predicate::new(PredicateType::RelatedTo), b);
        kg.add_triple(t);

        let stored = kg.get_entity("e1").unwrap();
        let cites = stored
            .metadata
            .get(CITATIONS_KEY)
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(
            cites.len(),
            1,
            "endpoint overwrite must keep prior citations"
        );
    }

    #[test]
    fn union_into_empty_dst_copies_all() {
        let mut a = HashMap::new();
        let mut b = HashMap::new();
        attach_citation(&mut b, &Citation::new(None, 2, 2));
        union_citations(&mut a, &b);
        assert!(a.contains_key(CITATIONS_KEY));
    }

    #[test]
    fn stamp_whole_document_covers_full_line_range_on_every_record() {
        use crate::types::{Entity, EntityType, Predicate, PredicateType, Triple};

        // 3 lines -> the whole-document citation must span [1, 3].
        let text = "alpha line\nbeta line\ngamma line";
        assert_eq!(LineIndex::new(text).total_lines(), 3);

        let mut kg = KnowledgeGraph::new();
        kg.add_entity(Entity::new("e1", "Alpha", EntityType::Organization));
        kg.add_entity(Entity::new("e2", "Beta", EntityType::Technology));
        kg.add_triple(Triple::new(
            Entity::new("e1", "Alpha", EntityType::Organization),
            Predicate::new(PredicateType::RelatedTo),
            Entity::new("e2", "Beta", EntityType::Technology),
        ));

        stamp_whole_document(&mut kg, &Some("doc.md".into()), text);

        let expected = json!({"doc": "doc.md", "lines": [1, 3]});
        for e in kg.entities.values() {
            assert_eq!(
                e.metadata
                    .get(CITATIONS_KEY)
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first()),
                Some(&expected),
                "entity {} must carry the whole-doc citation",
                e.label
            );
        }
        for t in &kg.triples {
            assert_eq!(
                t.metadata
                    .get(CITATIONS_KEY)
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first()),
                Some(&expected),
                "triple must carry the whole-doc citation"
            );
            for endpoint in [&t.subject, &t.object] {
                assert_eq!(
                    endpoint
                        .metadata
                        .get(CITATIONS_KEY)
                        .and_then(|v| v.as_array())
                        .and_then(|a| a.first()),
                    Some(&expected),
                    "triple endpoint {} must carry the whole-doc citation",
                    endpoint.label
                );
            }
        }
    }
}
