use super::*;

#[test]
fn test_no_collapse_needed() {
    let collapser = SmartCollapser::new(1000);
    let chunk = ChunkSpan {
        content: "fn foo() { bar(); }".to_string(),
        start_line: 0,
        end_line: 0,
        is_overview: false,
    };

    let result = collapser.collapse(&chunk);
    assert_eq!(result.content, chunk.content);
}

#[test]
fn test_collapse_block() {
    let content = r#"fn main() {
    let x = 1;
    let y = 2;
    println!("{}", x + y);
}"#;
    let result = collapse_block(content);
    assert_eq!(result, "fn main() { ... }");
}

#[test]
fn test_needs_collapsing() {
    let chunk = ChunkSpan {
        content: "...".to_string(),
        start_line: 0,
        end_line: 50,
        is_overview: false,
    };

    assert!(needs_collapsing(&chunk, 30));
    assert!(!needs_collapsing(&chunk, 100));
}

#[test]
fn test_collapse_nested() {
    let collapser = SmartCollapser::new(50);
    let chunk = ChunkSpan {
        content: "fn outer() { fn inner() { very_long_code_here(); } }".to_string(),
        start_line: 0,
        end_line: 0,
        is_overview: false,
    };

    let result = collapser.collapse(&chunk);
    assert!(result.content.contains("{ ... }"));
    assert!(result.content.len() <= 60); // Allow some overhead
}

#[test]
fn test_collapse_ignores_braces_in_strings() {
    // Use a large enough max_size to avoid truncation
    let collapser = SmartCollapser::new(500);

    // Code with braces inside a string - should NOT be counted as nesting
    let code_with_string = r#"fn has_string() {
    let s = "contains { braces }";
    process(s);
}"#;
    let chunk = ChunkSpan {
        content: code_with_string.to_string(),
        start_line: 0,
        end_line: 0,
        is_overview: false,
    };

    let result = collapser.collapse(&chunk);
    // The string with braces should be preserved, not collapsed
    assert!(
        result.content.contains(r#""contains { braces }""#),
        "String content with braces should be preserved. Got: {}",
        result.content
    );
}

#[test]
fn test_collapse_ignores_braces_in_comments() {
    let collapser = SmartCollapser::new(100);

    // Code with braces inside comments
    let code_with_comment = r#"fn has_comment() {
    // This is a comment with { braces }
    let x = 1;
}"#;
    let chunk = ChunkSpan {
        content: code_with_comment.to_string(),
        start_line: 0,
        end_line: 0,
        is_overview: false,
    };

    let result = collapser.collapse(&chunk);
    // The comment with braces should be preserved
    assert!(
        result
            .content
            .contains("// This is a comment with { braces }"),
        "Comment with braces should be preserved. Got: {}",
        result.content
    );
}

#[test]
fn test_collapse_ignores_braces_in_block_comments() {
    let collapser = SmartCollapser::new(100);

    // Code with braces inside block comments
    let code_with_block_comment = r#"fn has_block_comment() {
    /* Block comment with { braces } inside */
    let x = 1;
}"#;
    let chunk = ChunkSpan {
        content: code_with_block_comment.to_string(),
        start_line: 0,
        end_line: 0,
        is_overview: false,
    };

    let result = collapser.collapse(&chunk);
    // The block comment with braces should be preserved
    assert!(
        result
            .content
            .contains("/* Block comment with { braces } inside */"),
        "Block comment with braces should be preserved. Got: {}",
        result.content
    );
}
