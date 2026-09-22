//! Text segmentation backed by the `chonkie` crate.
//!
//! Replaces the Python `BaseExtractor.segment_chunks` (a hand-rolled character
//! sliding window). `ChunkStrategy::Char` reproduces that behaviour 1:1 via
//! chonkie's `CharChunker`; `Recursive`/`Token` are strictly-better upgrades
//! that respect word/sentence/token boundaries.

use crate::types::ChunkStrategy;
use chonkie::{CharChunker, Chunker, RecursiveChunker, TiktokenTokenizer, TokenChunker};
use core_types_rs::{BBox, CharSpan, LineSpan, PageSpan, SourceRange};
use serde_json::Value;
use std::collections::BTreeMap;

/// A single segment of text plus its char offsets in the combined input.
#[derive(Debug, Clone)]
pub struct Segment {
    pub content: String,
    pub index: usize,
    pub start: usize,
    pub end: usize,
    /// Complete protocol provenance for this segment. Plain-text chunking
    /// starts with a char span and lets extractors derive its line span;
    /// pre-chunked input preserves every supplied range coordinate, including
    /// page and bbox.
    pub range: Option<SourceRange>,
    /// Optional chunk title from pre-chunked input (kg-multimodal puts the
    /// generated item name here). `None` for plain-text chunking. Chunk-aware
    /// extractors stamp it onto the records extracted from this segment.
    pub title: Option<String>,
    /// Arbitrary chunk metadata from pre-chunked input (kg-multimodal's
    /// `mm_*` provenance keys, chonkie's `token_count`, …). Empty for
    /// plain-text chunking.
    pub metadata: BTreeMap<String, Value>,
}

impl Segment {
    /// Add a derived 1-based line span without replacing char/page/bbox
    /// provenance already carried by the segment.
    pub fn set_line_span(&mut self, start: usize, end: usize) {
        let line = u32::try_from(start)
            .ok()
            .zip(u32::try_from(end).ok())
            .and_then(|(start, end)| LineSpan::new(start, end));
        let char_span = CharSpan::new(self.start, self.end);
        self.range
            .get_or_insert_with(|| SourceRange {
                char_span,
                ..SourceRange::default()
            })
            .line = line;
    }

    /// Return the range when it carries evidence coordinates. A synthesized
    /// char span alone retains chunk positioning but preserves the historical
    /// behavior that chunks without line/spatial metadata are not cited.
    pub fn evidence_range(&self) -> Option<&SourceRange> {
        self.range
            .as_ref()
            .filter(|range| range.line.is_some() || range.page.is_some() || range.bbox.is_some())
    }

    pub fn line_span(&self) -> Option<LineSpan> {
        self.range.as_ref().and_then(|range| range.line)
    }
}

/// Segment `text` according to `strategy`. `segment_size`/`overlap` are used by
/// `Char` and `Token`; `Recursive` uses chonkie's default recursive rules sized
/// to `segment_size`.
pub fn segment(
    text: &str,
    strategy: ChunkStrategy,
    segment_size: usize,
    overlap: usize,
) -> Vec<Segment> {
    let chunks = match strategy {
        ChunkStrategy::Char => CharChunker::new(segment_size, overlap).chunk(text),
        ChunkStrategy::Token => {
            TokenChunker::new(segment_size, overlap, TiktokenTokenizer::new("gpt-4")).chunk(text)
        }
        ChunkStrategy::Recursive => {
            // Honor the caller's segment_size (chonkie's default is 2048 tokens,
            // which would ignore the configured size). Recursive has no overlap
            // knob — it merges splits up to chunk_size.
            RecursiveChunker {
                chunk_size: segment_size,
                ..RecursiveChunker::default()
            }
            .chunk(text)
        }
    };
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let (start, end) = c
                .range
                .as_ref()
                .and_then(|r| r.char_span.as_ref())
                .map(|cs| (cs.start, cs.end))
                .unwrap_or((0, 0));
            // chonkie and kg-extract can resolve core-types-rs through
            // different Cargo source IDs. Copy the public fields explicitly so
            // Segment owns kg-extract's protocol type while preserving the
            // complete range.
            let range = c.range.as_ref().map(|r| SourceRange {
                char_span: r
                    .char_span
                    .as_ref()
                    .and_then(|span| CharSpan::new(span.start, span.end)),
                line: r
                    .line
                    .as_ref()
                    .and_then(|span| LineSpan::new(span.start, span.end)),
                page: r
                    .page
                    .as_ref()
                    .and_then(|span| PageSpan::new(span.start, span.end)),
                bbox: r
                    .bbox
                    .as_ref()
                    .map(|bbox| BBox::new(bbox.x0, bbox.y0, bbox.x1, bbox.y1)),
            });
            Segment {
                content: c.text.unwrap_or_default(),
                index: i,
                start,
                end,
                range,
                title: None,
                metadata: BTreeMap::new(),
            }
        })
        .collect()
}

/// Pre-chunked input parsed from chonkie's serialized chunk output.
#[derive(Debug)]
pub struct PreChunked {
    pub segments: Vec<Segment>,
    /// `source_file` carried by the protocol chunks (the original document the
    /// chunks were cut from), if present — chonkie's CLI records it when run
    /// with `--with-source`. `"<stdin>"` is treated as unknown.
    pub source: Option<String>,
}

/// Parse chonkie chunk output — a JSON array (`chonkie --json`), a
/// `{"chunks": [...]}` truncation wrapper, or JSONL (`chonkie --jsonl`, whose
/// optional trailing `{"truncated": ...}` metadata line is skipped) — into
/// segments the extractors can consume *without re-chunking*.
///
/// Each chunk needs a `text` field. Character offsets are read from
/// `range.char_span.start`/`end` (protocol format); if absent, synthesized
/// cumulatively so offsets stay monotonic. Line ranges are read from
/// `range.line.start`/`end`; source file from `source_file`. The optional
/// `title` and `metadata` (a JSON object, e.g. kg-multimodal's `mm_*`
/// provenance keys) are preserved on the segment so chunk-aware extractors
/// can stamp them onto the records extracted from that chunk.
pub fn parse_prechunked(input: &str) -> anyhow::Result<PreChunked> {
    let input = input.trim();
    if input.is_empty() {
        anyhow::bail!("no pre-chunked input (expected chonkie JSON or JSONL chunks)");
    }

    // One JSON document (array, or an object wrapper / single chunk)?
    // Otherwise fall back to JSONL, one chunk object per line.
    let values: Vec<Value> = match serde_json::from_str::<Value>(input) {
        Ok(Value::Array(arr)) => arr,
        Ok(Value::Object(obj)) => match obj.get("chunks").and_then(Value::as_array) {
            Some(arr) => arr.clone(),
            None => vec![Value::Object(obj)],
        },
        Ok(other) => anyhow::bail!("pre-chunked input must be chunk objects, got: {other}"),
        Err(_) => input
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .enumerate()
            .map(|(i, l)| {
                serde_json::from_str::<Value>(l)
                    .map_err(|e| anyhow::anyhow!("pre-chunked input line {}: {e}", i + 1))
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
    };

    let mut segments = Vec::new();
    let mut source: Option<String> = None;
    let mut cursor = 0usize; // synthesized offset when a chunk omits indices
    for (i, v) in values.into_iter().enumerate() {
        let text = match v.get("text").and_then(Value::as_str) {
            Some(t) => t.to_string(),
            // chonkie --jsonl --limit appends a `{"truncated": ...}` trailer.
            None if v.get("truncated").is_some() => continue,
            None => anyhow::bail!("chunk {} has no \"text\" field: {v}", i + 1),
        };

        // Protocol format: range.char_span.start / range.char_span.end
        let char_span = v.get("range").and_then(|r| r.get("char_span"));
        let start = char_span
            .and_then(|cs| cs.get("start"))
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(cursor);
        let end = char_span
            .and_then(|cs| cs.get("end"))
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or_else(|| start + text.chars().count());
        if start > end {
            anyhow::bail!(
                "chunk {} has invalid char span: start {start} exceeds end {end}",
                i + 1
            );
        }
        cursor = end;

        // Deserialize the protocol range as one unit so new first-class
        // coordinates (for example page and bbox) cannot be accidentally
        // discarded by a field-by-field parser.
        let mut range = v
            .get("range")
            .filter(|value| !value.is_null())
            .map(|value| serde_json::from_value::<SourceRange>(value.clone()))
            .transpose()
            .map_err(|e| anyhow::anyhow!("chunk {} has invalid range: {e}", i + 1))?
            .unwrap_or_default();
        range.char_span = CharSpan::new(start, end);

        // `PreChunked` represents one source document. Reject conflicting
        // source labels instead of silently attributing every fact to the
        // first chunk's file — and say exactly which chunk conflicts and how
        // to recover, so a multi-document producer (e.g. a kg-multimodal
        // sidecar spanning several files) is fixable without guesswork.
        let chunk_source = v
            .get("source_file")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && *s != "<stdin>");
        if let Some(chunk_source) = chunk_source {
            match source.as_deref() {
                Some(existing) if existing != chunk_source => anyhow::bail!(
                    "chunk {} has source_file {chunk_source:?} but earlier chunks came from \
                     {existing:?}; kg-extract extracts one source document per run — split the \
                     input by source_file and run once per document",
                    i + 1
                ),
                Some(_) => {}
                None => source = Some(chunk_source.to_string()),
            }
        }

        // Optional per-chunk payload: a title (kg-multimodal's generated item
        // name) and a free-form metadata object. Both are stamped onto the
        // records the chunk-aware extractors derive from this chunk, so they
        // reach protocol properties instead of being silently dropped.
        let title = v
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty());
        let metadata = match v.get("metadata") {
            None | Some(Value::Null) => BTreeMap::new(),
            Some(Value::Object(obj)) => obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            Some(_) => anyhow::bail!(
                "chunk {} has invalid metadata (expected a JSON object)",
                i + 1
            ),
        };

        segments.push(Segment {
            content: text,
            index: segments.len(),
            start,
            end,
            range: Some(range),
            title,
            metadata,
        });
    }

    if segments.is_empty() {
        anyhow::bail!("pre-chunked input contains no chunks");
    }
    Ok(PreChunked { segments, source })
}

/// Join chunk contents with a separator (mirrors `chunks_to_text`).
pub fn join_texts<'a>(parts: impl IntoIterator<Item = &'a str>, separator: &str) -> String {
    parts.into_iter().collect::<Vec<_>>().join(separator)
}

#[cfg(test)]
#[path = "chunking_tests.rs"]
mod tests;
