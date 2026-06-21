use super::*;

#[test]
fn test_small_content_single_chunk() {
    let content = "# Title\n\nSmall content.";
    let chunker = MarkdownChunker::new(1000);
    let chunks = chunker.chunk(content);

    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].content, content);
    assert_eq!(chunks[0].start_line, 1);
}

#[test]
fn test_split_by_h2_headers() {
    let content = r#"# Main Title

Intro text.

## Section 1

Content of section 1.

## Section 2

Content of section 2.
"#;
    let chunker = MarkdownChunker::new(50);
    let chunks = chunker.chunk(content);

    // Should create multiple chunks based on h2 headers
    assert!(chunks.len() >= 2);

    // Each section chunk should include its header
    let combined: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert!(combined.contains("## Section 1"));
    assert!(combined.contains("## Section 2"));
}

#[test]
fn test_nested_headers() {
    let content = r#"# Top

## Sub1

### SubSub1

Content here.

## Sub2

More content."#;
    let chunker = MarkdownChunker::new(30);
    let chunks = chunker.chunk(content);

    // Should have chunks with nested headers preserved
    assert!(!chunks.is_empty());
}

#[test]
fn test_clean_fragment() {
    assert_eq!(clean_fragment("# Hello World"), "hello-world");
    assert_eq!(clean_fragment("## API Reference (v2)"), "api-reference-v2");
    assert_eq!(
        clean_fragment("### Link [Example](http://example.com)"),
        "link-example"
    );
    assert_eq!(
        clean_fragment("Special $chars% here!"),
        "special-chars-here"
    );
}

#[test]
fn test_is_markdown_file() {
    assert!(is_markdown_file("md"));
    assert!(is_markdown_file("MD"));
    assert!(is_markdown_file("markdown"));
    assert!(is_markdown_file("mdx"));
    assert!(!is_markdown_file("txt"));
    assert!(!is_markdown_file("rs"));
}

#[test]
fn test_line_numbers() {
    let content = "# Title\n\nLine 3\nLine 4\nLine 5";
    let chunker = MarkdownChunker::new(1000);
    let chunks = chunker.chunk(content);

    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].start_line, 1);
    assert_eq!(chunks[0].end_line, 5);
}

#[test]
fn test_fallback_to_line_chunking() {
    // Content with no headers should still be chunked
    let content = "Line 1\n".repeat(100);
    let chunker = MarkdownChunker::new(50);
    let chunks = chunker.chunk(&content);

    assert!(chunks.len() > 1);
}
