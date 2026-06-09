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
}

/// Segment `text` according to `strategy`. `segment_size`/`overlap` are used by
/// `Char` and `Token`; `Recursive` uses chonkie's default recursive rules sized
/// to `segment_size`.
pub fn segment(text: &str, strategy: ChunkStrategy, segment_size: usize, overlap: usize) -> Vec<Segment> {
    let chunks = match strategy {
        ChunkStrategy::Char => CharChunker::new(segment_size, overlap).chunk(text),
        ChunkStrategy::Token => {
            TokenChunker::new(segment_size, overlap, TiktokenTokenizer::new("gpt-4")).chunk(text)
        }
        ChunkStrategy::Recursive => {
            // Honor the caller's segment_size (chonkie's default is 2048 tokens,
            // which would ignore the configured size). Recursive has no overlap
            // knob — it merges splits up to chunk_size.
            RecursiveChunker { chunk_size: segment_size, ..RecursiveChunker::default() }.chunk(text)
        }
    };
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, c)| Segment {
            content: c.text,
            index: i,
            start: c.start_index,
            end: c.end_index,
        })
        .collect()
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
        assert!(segs.len() >= 10, "char chunker splits by size, got {}", segs.len());
    }
}
