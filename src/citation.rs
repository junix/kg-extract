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
//!
//! A parallel chunk-level channel carries pre-chunked *payloads*: a chunk's
//! optional `title`/`metadata` (e.g. kg-multimodal's `mm_*` provenance keys)
//! is stamped onto every record extracted from that chunk under
//! [`CHUNK_TITLE_KEY`] / [`CHUNK_METADATA_KEY`] (see [`stamp_chunk_metadata`]),
//! so it survives the input boundary into protocol properties.

use std::collections::HashMap;

use core_types_rs::{LineSpan, SourceRange};
use serde_json::{json, Value};

use crate::types::KnowledgeGraph;

/// Metadata key under which citation arrays live.
pub const CITATIONS_KEY: &str = "citations";

/// Metadata key under which a record's source-chunk title lives (pre-chunked
/// input only; kg-multimodal chunks carry the generated item name here).
pub const CHUNK_TITLE_KEY: &str = "chunk_title";

/// Metadata key under which a record's source-chunk metadata object lives
/// (pre-chunked input only — e.g. kg-multimodal's `mm_*` provenance keys).
pub const CHUNK_METADATA_KEY: &str = "chunk_metadata";

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

/// Attach a pre-chunked segment's `title`/`metadata` to one record's metadata
/// map under [`CHUNK_TITLE_KEY`] / [`CHUNK_METADATA_KEY`]. This is the
/// chunk-level provenance channel parallel to citations: chunk-aware
/// extractors stamp the payload of the chunk a record was extracted from, so
/// producer metadata (kg-multimodal's `mm_*` keys, the chunk title) survives
/// into protocol properties instead of being dropped at the input boundary.
/// A no-op for segments that carry neither (plain-text chunking).
pub fn attach_chunk_metadata(
    metadata: &mut HashMap<String, Value>,
    segment: &crate::chunking::Segment,
) {
    if let Some(title) = &segment.title {
        metadata.insert(CHUNK_TITLE_KEY.to_string(), json!(title));
    }
    if !segment.metadata.is_empty() {
        metadata.insert(
            CHUNK_METADATA_KEY.to_string(),
            Value::Object(
                segment
                    .metadata
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
        );
    }
}

/// Stamp a pre-chunked segment's title/metadata onto every entity and triple
/// of `kg` — same coverage as [`stamp_graph`], endpoint snapshots included.
pub fn stamp_chunk_metadata(kg: &mut KnowledgeGraph, segment: &crate::chunking::Segment) {
    if segment.title.is_none() && segment.metadata.is_empty() {
        return;
    }
    for e in kg.entities.values_mut() {
        attach_chunk_metadata(&mut e.metadata, segment);
    }
    for t in kg.triples.iter_mut() {
        attach_chunk_metadata(&mut t.metadata, segment);
        attach_chunk_metadata(&mut t.subject.metadata, segment);
        attach_chunk_metadata(&mut t.object.metadata, segment);
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
#[path = "citation_tests.rs"]
mod tests;
