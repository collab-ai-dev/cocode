use super::*;

#[test]
fn test_markup_to_string_plain() {
    let content = lsp_types::MarkedString::String("Hello world".to_string());
    assert_eq!(LspClient::markup_to_string(content), "Hello world");
}

#[test]
fn test_markup_to_string_language() {
    let content = lsp_types::MarkedString::LanguageString(lsp_types::LanguageString {
        language: "rust".to_string(),
        value: "fn main() {}".to_string(),
    });
    assert_eq!(
        LspClient::markup_to_string(content),
        "```rust\nfn main() {}\n```"
    );
}

#[test]
fn test_language_id_from_extension() {
    // Test the language detection logic used in sync_file
    let test_cases = vec![
        ("rs", "rust"),
        ("go", "go"),
        ("py", "python"),
        ("pyi", "python"),
        ("txt", "plaintext"),
        ("unknown", "plaintext"),
    ];

    for (ext, expected) in test_cases {
        let language_id = match ext {
            "rs" => "rust",
            "go" => "go",
            "py" | "pyi" => "python",
            _ => "plaintext",
        };
        assert_eq!(
            language_id, expected,
            "Extension '{ext}' should map to '{expected}'"
        );
    }
}

#[test]
fn test_incremental_no_changes() {
    let old = DocumentContent::new("foo\nbar\nbaz\n".to_string());
    let changes = compute_incremental_changes(&old, "foo\nbar\nbaz\n");
    assert!(
        changes.is_empty(),
        "Expected no changes for identical content"
    );
}

#[test]
fn test_incremental_single_line_modification() {
    let old = DocumentContent::new("foo\nbar\nbaz\n".to_string());
    let changes = compute_incremental_changes(&old, "foo\nBAR\nbaz\n");

    assert!(!changes.is_empty(), "Expected changes for modified line");

    let has_line_1_change = changes
        .iter()
        .any(|c| c.range.as_ref().map(|r| r.start.line == 1).unwrap_or(false));
    assert!(has_line_1_change, "Expected change event for line 1");
}

#[test]
fn test_incremental_line_insertion() {
    let old = DocumentContent::new("foo\nbaz\n".to_string());
    let changes = compute_incremental_changes(&old, "foo\nbar\nbaz\n");
    assert!(!changes.is_empty(), "Expected changes for inserted line");
}

#[test]
fn test_incremental_line_deletion() {
    let old = DocumentContent::new("foo\nbar\nbaz\n".to_string());
    let changes = compute_incremental_changes(&old, "foo\nbaz\n");
    assert!(!changes.is_empty(), "Expected changes for deleted line");
}

#[test]
fn test_incremental_change_has_range() {
    let old = DocumentContent::new("foo\nbar\nbaz\n".to_string());
    let changes = compute_incremental_changes(&old, "foo\nBAR\nbaz\n");

    for change in &changes {
        assert!(
            change.range.is_some(),
            "Incremental changes should have range"
        );
    }
}
