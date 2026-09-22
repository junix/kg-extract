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
fn prechunked_preserves_title_and_metadata() {
    let input = r#"
{"id":"tbl-1","text":"Revenue grew to $3.8M.","source_file":"doc.pdf","title":"q4_revenue","metadata":{"mm_kind":"table","mm_table_format":"html"}}
{"id":"chnk_2","text":"Plain chunk."}
"#;
    let p = parse_prechunked(input).unwrap();
    let s = &p.segments[0];
    assert_eq!(s.title.as_deref(), Some("q4_revenue"));
    assert_eq!(s.metadata.get("mm_kind").unwrap(), "table");
    assert_eq!(s.metadata.get("mm_table_format").unwrap(), "html");
    // A chunk without the optional payload stays empty, not an error.
    assert_eq!(p.segments[1].title, None);
    assert!(p.segments[1].metadata.is_empty());
}

#[test]
fn prechunked_rejects_non_object_metadata() {
    let input = r#"{"text":"A","metadata":"not-an-object"}"#;
    let error = parse_prechunked(input).unwrap_err().to_string();
    assert!(error.contains("metadata"), "got: {error}");
}

#[test]
fn prechunked_rejects_conflicting_source_files() {
    let input = r#"
{"text":"A","source_file":"one.md"}
{"text":"B","source_file":"two.md"}
"#;
    let error = parse_prechunked(input).unwrap_err().to_string();
    assert!(error.contains("one.md") && error.contains("two.md"));
    // Actionable: names the offending chunk and how to recover.
    assert!(error.contains("chunk 2"), "got: {error}");
    assert!(
        error.contains("split the input by source_file"),
        "got: {error}"
    );
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
