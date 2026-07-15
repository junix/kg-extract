//! Text segmentation backed by the `chonkie` crate.
//!
//! Replaces the Python `BaseExtractor.segment_chunks` (a hand-rolled character
//! sliding window). `ChunkStrategy::Char` reproduces that behaviour 1:1 via
//! chonkie's `CharChunker`; `Recursive`/`Token` are strictly-better upgrades
//! that respect word/sentence/token boundaries.

use crate::types::ChunkStrategy;
use chonkie::{CharChunker, Chunker, RecursiveChunker, TiktokenTokenizer, TokenChunker};
use core_types_rs::{BBox, CharSpan, LineSpan, PageSpan, SourceRange};

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
/// `range.line.start`/`end`; source file from `source_file`.
pub fn parse_prechunked(input: &str) -> anyhow::Result<PreChunked> {
    use serde_json::Value;

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
        // first chunk's file.
        let chunk_source = v
            .get("source_file")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && *s != "<stdin>");
        if let Some(chunk_source) = chunk_source {
            match source.as_deref() {
                Some(existing) if existing != chunk_source => anyhow::bail!(
                    "pre-chunked input mixes source files {existing:?} and {chunk_source:?}"
                ),
                Some(_) => {}
                None => source = Some(chunk_source.to_string()),
            }
        }

        segments.push(Segment {
            content: text,
            index: segments.len(),
            start,
            end,
            range: Some(range),
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
mod tests {
    use super::*;

    #[test]
    fn recursive_honors_segment_size() {
        // ~280 words: many tokens, but far under chonkie's 2048-token default.
        // With the default size this is one chunk; honoring segment_size must
        // split it into several.
        let text = "Knowledge graphs link entities through typed relations. ".repeat(40);
        let segs = segment(&text, ChunkStrategy::Recursive, 32, 0);
        assert!(
            segs.len() > 1,
            "recursive chunker must respect segment_size, got {} segment(s)",
            segs.len()
        );
    }

    #[test]
    fn char_segment_size_splits() {
        let text = "x".repeat(1000);
        let segs = segment(&text, ChunkStrategy::Char, 100, 0);
        // 1000 chars / size 100 / overlap 0 is exactly 10 deterministic segments.
        assert_eq!(
            segs.len(),
            10,
            "char chunker splits by size, got {}",
            segs.len()
        );
    }

    #[test]
    fn prechunked_parses_jsonl_with_metadata() {
        let input = r#"
{"id":"chnk_1","text":"First chunk.","source_file":"doc.md","range":{"char_span":{"start":0,"end":12},"line":{"start":1,"end":2}},"metadata":{"token_count":3}}
{"id":"chnk_2","text":"Second chunk.","source_file":"doc.md","range":{"char_span":{"start":12,"end":25},"line":{"start":3,"end":4}},"metadata":{"token_count":3}}
"#;
        let p = parse_prechunked(input).unwrap();
        assert_eq!(p.segments.len(), 2);
        assert_eq!(p.source.as_deref(), Some("doc.md"));
        let s = &p.segments[1];
        assert_eq!(s.content, "Second chunk.");
        assert_eq!((s.index, s.start, s.end), (1, 12, 25));
        assert_eq!(
            s.range.as_ref().and_then(|range| range.line),
            LineSpan::new(3, 4)
        );
    }

    #[test]
    fn prechunked_parses_json_array_and_truncation_wrapper() {
        let arr = r#"[{"text":"A","range":{"char_span":{"start":0,"end":1}}},{"text":"B","range":{"char_span":{"start":1,"end":2}}}]"#;
        assert_eq!(parse_prechunked(arr).unwrap().segments.len(), 2);

        // `chonkie --json --limit` wraps the chunks when truncated.
        let wrapped = r#"{"chunks":[{"text":"A","range":{"char_span":{"start":0,"end":1}}}],"truncated":true,"returned":1,"total":9}"#;
        let p = parse_prechunked(wrapped).unwrap();
        assert_eq!(p.segments.len(), 1);
        assert_eq!(p.segments[0].content, "A");
    }

    #[test]
    fn prechunked_skips_jsonl_truncation_trailer() {
        let input = "{\"text\":\"A\",\"range\":{\"char_span\":{\"start\":0,\"end\":1}}}\n\
            {\"truncated\":true,\"returned\":1,\"total\":5}";
        let p = parse_prechunked(input).unwrap();
        assert_eq!(p.segments.len(), 1);
    }

    #[test]
    fn prechunked_synthesizes_missing_offsets_cumulatively() {
        let input = "{\"text\":\"abcde\"}\n{\"text\":\"fgh\"}";
        let p = parse_prechunked(input).unwrap();
        assert_eq!((p.segments[0].start, p.segments[0].end), (0, 5));
        assert_eq!((p.segments[1].start, p.segments[1].end), (5, 8));
        assert_eq!(
            p.segments[1].range.as_ref().and_then(|range| range.line),
            None
        );
    }

    #[test]
    fn prechunked_stdin_source_is_unknown() {
        let input = r#"{"text":"A","source_file":"<stdin>"}"#;
        assert!(parse_prechunked(input).unwrap().source.is_none());
    }

    #[test]
    fn prechunked_rejects_conflicting_source_files() {
        let input = r#"
{"text":"A","source_file":"one.md"}
{"text":"B","source_file":"two.md"}
"#;
        let error = parse_prechunked(input).unwrap_err().to_string();
        assert!(error.contains("one.md") && error.contains("two.md"));
    }

    #[test]
    fn prechunked_rejects_inverted_char_span() {
        let input = r#"{"text":"A","range":{"char_span":{"start":5,"end":2}}}"#;
        let error = parse_prechunked(input).unwrap_err().to_string();
        assert!(error.contains("char span") && error.contains("5") && error.contains("2"));
    }

    #[test]
    fn prechunked_rejects_chunk_without_text_and_empty_input() {
        assert!(parse_prechunked(r#"{"id":"x"}"#).is_err());
        assert!(parse_prechunked("").is_err());
        assert!(parse_prechunked("[]").is_err());
        assert!(parse_prechunked("not json at all").is_err());
    }
}
