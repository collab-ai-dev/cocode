use super::*;
use crate::tags::extractor::CodeTag;
use crate::tags::extractor::TagKind;

fn make_symbol(name: &str, line: i32, signature: &str) -> RankedSymbol {
    RankedSymbol {
        tag: CodeTag {
            name: name.to_string(),
            kind: TagKind::Function,
            start_line: line,
            end_line: line + 10,
            start_byte: line * 100,
            end_byte: (line + 10) * 100,
            signature: Some(signature.to_string()),
            docs: None,
            is_definition: true,
        },
        rank: 1.0 / (line as f64),
        filepath: format!("src/file_{}.rs", line / 100),
    }
}

#[test]
fn test_render_empty() {
    let renderer = TreeRenderer::new();
    let output = renderer.render_symbols(&[], 10);
    assert!(output.is_empty());
}

#[test]
fn test_render_single_symbol() {
    let renderer = TreeRenderer::new();
    let symbols = vec![make_symbol("foo", 10, "fn foo() -> i32")];

    let output = renderer.render_symbols(&symbols, 1);

    assert!(output.contains("fn foo() -> i32"));
    assert!(output.contains("10:"));
}

#[test]
fn test_render_multiple_symbols() {
    let renderer = TreeRenderer::new();
    let symbols = vec![
        make_symbol("foo", 10, "fn foo()"),
        make_symbol("bar", 20, "fn bar()"),
        make_symbol("baz", 30, "fn baz()"),
    ];

    let output = renderer.render_symbols(&symbols, 3);

    assert!(output.contains("fn foo()"));
    assert!(output.contains("fn bar()"));
    assert!(output.contains("fn baz()"));
}

#[test]
fn test_render_with_count_limit() {
    let renderer = TreeRenderer::new();
    let symbols = vec![
        make_symbol("foo", 10, "fn foo()"),
        make_symbol("bar", 20, "fn bar()"),
        make_symbol("baz", 30, "fn baz()"),
    ];

    let output = renderer.render_symbols(&symbols, 2);

    assert!(output.contains("fn foo()"));
    assert!(output.contains("fn bar()"));
    assert!(!output.contains("fn baz()"));
}

#[test]
fn test_render_without_line_numbers() {
    let renderer = TreeRenderer::with_options(false, true);
    let symbols = vec![make_symbol("foo", 10, "fn foo()")];

    let output = renderer.render_symbols(&symbols, 1);

    assert!(output.contains("fn foo()"));
    assert!(!output.contains("10:"));
}

#[test]
fn test_render_full_tree() {
    let renderer = TreeRenderer::new();
    let symbols = vec![
        make_symbol("process", 100, "fn process(req: Request) -> Response"),
        make_symbol("handle", 150, "fn handle(data: &[u8])"),
    ];

    let chat_files: HashSet<String> = ["src/file_1.rs".to_string()].into_iter().collect();
    let (output, rendered_files) = renderer.render(&symbols, &chat_files, 2, Path::new("/project"));

    // Should have file headers and symbol lines
    assert!(output.contains(".rs:"));
    assert!(output.contains("fn process"));

    // Should return the set of rendered files
    assert!(rendered_files.contains("src/file_1.rs"));
}

#[test]
fn test_line_truncation() {
    // Test that lines longer than MAX_LINE_LENGTH are truncated
    let long_line = "a".repeat(150);
    let truncated = TreeRenderer::truncate_lines(&long_line);

    // Should be MAX_LINE_LENGTH chars (97 + "...")
    assert_eq!(truncated.len(), 100);
    assert!(truncated.ends_with("..."));

    // Short lines should not be truncated
    let short_line = "short line";
    let not_truncated = TreeRenderer::truncate_lines(short_line);
    assert_eq!(not_truncated, short_line);

    // Test multiple lines
    let multi_line = format!("{}\n{}\nshort", "b".repeat(120), "c".repeat(80));
    let truncated_multi = TreeRenderer::truncate_lines(&multi_line);
    let lines: Vec<&str> = truncated_multi.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].ends_with("...")); // First line truncated
    assert_eq!(lines[1].len(), 80); // Second line not truncated (80 < 100)
    assert_eq!(lines[2], "short"); // Third line not truncated
}
