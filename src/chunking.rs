//! Text segmentation backed by the `chonkie` crate.
//!
//! Replaces the Python `BaseExtractor.segment_chunks` (a hand-rolled character
//! sliding window). `ChunkStrategy::Char` reproduces that behaviour 1:1 via
//! chonkie's `CharChunker`; `Recursive`/`Token` are strictly-better upgrades
//! that respect word/sentence/token boundaries.

use crate::types::ChunkStrategy;
use chonkie::{CharChunker, Chunker, RecursiveChunker, TiktokenTokenizer, TokenChunker};

/// A single segment of text plus its char offsets in the combined input.
#[derive(Debug, Clone)]
pub struct Segment {
    pub content: String,
    pub index: usize,
    pub start: usize,
    pub end: usize,
    /// 1-based inclusive line range of this segment in the source document,
    /// when known. [`segment`] leaves it `None` (extractors derive lines from
    /// the char offsets, which they can because they hold the full text);
    /// pre-chunked input carries it from chonkie's protocol `range.line`
    /// field, the only line information available without the text.
    pub lines: Option<(usize, usize)>,
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
            let lines = c.range.as_ref().and_then(|r| r.line.as_ref()).map(|l| {
                // LineSpan uses 0-based internally; convert to 1-based inclusive.
                (l.start as usize + 1, l.end as usize + 1)
            });
            Segment {
                content: c.text.unwrap_or_default(),
                index: i,
                start,
                end,
                lines,
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
        let char_span = v
            .get("range")
            .and_then(|r| r.get("char_span"));
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
        cursor = end;

        // Protocol format: range.line.start / range.line.end
        let range_obj = v.get("range");
        let line_val = |key: &str| {
            range_obj
                .and_then(|r| r.get("line"))
                .and_then(|l| l.get(key))
                .and_then(Value::as_u64)
                .map(|n| n as usize)
        };
        let lines = match (line_val("start"), line_val("end")) {
            (Some(s), Some(e)) => Some((s, e)),
            _ => None,
        };

        // Protocol format: source_file (top-level field)
        if source.is_none() {
            source = v
                .get("source_file")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty() && *s != "<stdin>")
                .map(str::to_string);
        }

        segments.push(Segment {
            content: text,
            index: segments.len(),
            start,
            end,
            lines,
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
        assert!(
            segs.len() >= 10,
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
        assert_eq!(s.lines, Some((3, 4)));
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
        assert_eq!(p.segments[1].lines, None);
    }

    #[test]
    fn prechunked_stdin_source_is_unknown() {
        let input = r#"{"text":"A","source_file":"<stdin>"}"#;
        assert!(parse_prechunked(input).unwrap().source.is_none());
    }

    #[test]
    fn prechunked_rejects_chunk_without_text_and_empty_input() {
        assert!(parse_prechunked(r#"{"id":"x"}"#).is_err());
        assert!(parse_prechunked("").is_err());
        assert!(parse_prechunked("[]").is_err());
        assert!(parse_prechunked("not json at all").is_err());
    }
}
