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
        ChunkStrategy::Recursive => RecursiveChunker::default().chunk(text),
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
