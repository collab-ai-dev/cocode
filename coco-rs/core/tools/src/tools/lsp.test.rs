use super::*;

#[test]
fn test_uri_to_file_path_unix() {
    assert_eq!(
        uri_to_file_path("file:///home/user/file.rs"),
        "/home/user/file.rs"
    );
    assert_eq!(uri_to_file_path("/home/user/file.rs"), "/home/user/file.rs");
}

#[test]
fn test_uri_to_file_path_percent_encoded() {
    assert_eq!(
        uri_to_file_path("file:///home/user/my%20file.rs"),
        "/home/user/my file.rs"
    );
}

#[test]
fn test_format_uri_absolute() {
    assert_eq!(
        format_uri("file:///home/user/project/src/main.rs", None),
        "/home/user/project/src/main.rs"
    );
}

#[test]
fn test_format_uri_relative() {
    let result = format_uri(
        "file:///home/user/project/src/main.rs",
        Some("/home/user/project"),
    );
    assert_eq!(result, "src/main.rs");
}

#[test]
fn test_format_uri_empty() {
    assert_eq!(format_uri("", None), "<unknown location>");
}

#[test]
fn test_format_definition_empty() {
    let result = format_definition_result(&[], None);
    assert!(result.starts_with("No definition found"));
}

#[test]
fn test_format_definition_single() {
    let loc = LspLocation {
        uri: "file:///src/main.rs".into(),
        range: LspRange {
            start: LspPosition {
                line: 9,
                character: 4,
            },
            end: LspPosition {
                line: 9,
                character: 10,
            },
        },
    };
    let result = format_definition_result(&[loc], None);
    assert!(result.starts_with("Defined in"));
    assert!(result.contains("10:5")); // 1-based
}

#[test]
fn test_format_definition_multiple() {
    let locs = vec![
        LspLocation {
            uri: "file:///src/a.rs".into(),
            range: LspRange {
                start: LspPosition {
                    line: 0,
                    character: 0,
                },
                end: LspPosition {
                    line: 0,
                    character: 5,
                },
            },
        },
        LspLocation {
            uri: "file:///src/b.rs".into(),
            range: LspRange {
                start: LspPosition {
                    line: 3,
                    character: 2,
                },
                end: LspPosition {
                    line: 3,
                    character: 8,
                },
            },
        },
    ];
    let result = format_definition_result(&locs, None);
    assert!(result.starts_with("Found 2 definitions:"));
}

#[test]
fn test_format_references_grouped_by_file() {
    let locs = vec![
        LspLocation {
            uri: "file:///src/main.rs".into(),
            range: LspRange {
                start: LspPosition {
                    line: 0,
                    character: 0,
                },
                end: LspPosition {
                    line: 0,
                    character: 5,
                },
            },
        },
        LspLocation {
            uri: "file:///src/main.rs".into(),
            range: LspRange {
                start: LspPosition {
                    line: 10,
                    character: 3,
                },
                end: LspPosition {
                    line: 10,
                    character: 8,
                },
            },
        },
        LspLocation {
            uri: "file:///src/lib.rs".into(),
            range: LspRange {
                start: LspPosition {
                    line: 5,
                    character: 0,
                },
                end: LspPosition {
                    line: 5,
                    character: 5,
                },
            },
        },
    ];
    let result = format_references_result(&locs, None);
    assert!(result.contains("3 references across 2 files"));
    assert!(result.contains("Line 1:1"));
    assert!(result.contains("Line 11:4"));
}

#[test]
fn test_format_hover_none() {
    let result = format_hover_result(None);
    assert!(result.starts_with("No hover information"));
}

#[test]
fn test_format_hover_with_range() {
    let hover = HoverResult {
        contents: HoverContents::String("fn foo() -> i32".into()),
        range: Some(LspRange {
            start: LspPosition {
                line: 4,
                character: 2,
            },
            end: LspPosition {
                line: 4,
                character: 5,
            },
        }),
    };
    let result = format_hover_result(Some(&hover));
    assert!(result.contains("Hover info at 5:3"));
    assert!(result.contains("fn foo() -> i32"));
}

#[test]
fn test_format_document_symbols_empty() {
    let result = format_document_symbols(&[]);
    assert_eq!(result, "No symbols found in document.");
}

#[test]
fn test_format_document_symbols_nested() {
    let symbols = vec![DocumentSymbol {
        name: "MyStruct".into(),
        kind: SymbolKind(23), // Struct
        range: LspRange {
            start: LspPosition {
                line: 0,
                character: 0,
            },
            end: LspPosition {
                line: 10,
                character: 0,
            },
        },
        detail: None,
        children: vec![DocumentSymbol {
            name: "new".into(),
            kind: SymbolKind(6), // Method
            range: LspRange {
                start: LspPosition {
                    line: 2,
                    character: 4,
                },
                end: LspPosition {
                    line: 5,
                    character: 4,
                },
            },
            detail: Some("() -> Self".into()),
            children: vec![],
        }],
    }];
    let result = format_document_symbols(&symbols);
    assert!(result.contains("MyStruct (Struct) - Line 1"));
    assert!(result.contains("  new (Method) () -> Self - Line 3"));
}

#[test]
fn test_format_workspace_symbols() {
    let symbols = vec![SymbolInformation {
        name: "Config".into(),
        kind: SymbolKind(23),
        location: LspLocation {
            uri: "file:///src/config.rs".into(),
            range: LspRange {
                start: LspPosition {
                    line: 5,
                    character: 0,
                },
                end: LspPosition {
                    line: 20,
                    character: 0,
                },
            },
        },
        container_name: Some("crate::config".into()),
    }];
    let result = format_workspace_symbols(&symbols, None);
    assert!(result.contains("Found 1 symbol"));
    assert!(result.contains("Config (Struct) - Line 6 in crate::config"));
}

#[test]
fn test_format_incoming_calls() {
    let calls = vec![IncomingCall {
        from: CallHierarchyItem {
            name: "main".into(),
            kind: SymbolKind(12),
            uri: "file:///src/main.rs".into(),
            range: LspRange {
                start: LspPosition {
                    line: 0,
                    character: 0,
                },
                end: LspPosition {
                    line: 10,
                    character: 0,
                },
            },
            detail: None,
        },
        from_ranges: vec![LspRange {
            start: LspPosition {
                line: 5,
                character: 4,
            },
            end: LspPosition {
                line: 5,
                character: 10,
            },
        }],
    }];
    let result = format_incoming_calls(&calls, None);
    assert!(result.contains("Found 1 incoming call"));
    assert!(result.contains("main (Function) - Line 1"));
    assert!(result.contains("[calls at: 6:5]"));
}

#[test]
fn test_format_outgoing_calls_empty() {
    let result = format_outgoing_calls(&[], None);
    assert!(result.contains("No outgoing calls found"));
}

#[test]
fn test_count_unique_files() {
    let locs = vec![
        LspLocation {
            uri: "file:///a.rs".into(),
            range: LspRange {
                start: LspPosition {
                    line: 0,
                    character: 0,
                },
                end: LspPosition {
                    line: 0,
                    character: 0,
                },
            },
        },
        LspLocation {
            uri: "file:///b.rs".into(),
            range: LspRange {
                start: LspPosition {
                    line: 0,
                    character: 0,
                },
                end: LspPosition {
                    line: 0,
                    character: 0,
                },
            },
        },
        LspLocation {
            uri: "file:///a.rs".into(),
            range: LspRange {
                start: LspPosition {
                    line: 5,
                    character: 0,
                },
                end: LspPosition {
                    line: 5,
                    character: 0,
                },
            },
        },
    ];
    assert_eq!(count_unique_files(&locs), 2);
}

#[test]
fn test_symbol_kind_labels() {
    assert_eq!(SymbolKind(5).label(), "Class");
    assert_eq!(SymbolKind(12).label(), "Function");
    assert_eq!(SymbolKind(23).label(), "Struct");
    assert_eq!(SymbolKind(99).label(), "Unknown");
}

#[test]
fn test_path_to_file_uri_absolute() {
    assert_eq!(
        path_to_file_uri(Path::new("/home/user/file.rs")),
        Some("file:///home/user/file.rs".to_string())
    );
}

#[test]
fn test_path_to_file_uri_percent_encodes_spaces() {
    assert_eq!(
        path_to_file_uri(Path::new("/home/user/my file.rs")),
        Some("file:///home/user/my%20file.rs".to_string())
    );
}

#[test]
fn test_path_to_file_uri_rejects_relative() {
    assert_eq!(path_to_file_uri(Path::new("relative/path.rs")), None);
}

#[test]
fn test_validate_lsp_file_nonexistent() {
    let result = validate_lsp_file(Path::new("/nonexistent/file.rs"), 10_000_000);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("does not exist"));
}

#[test]
fn test_validate_lsp_file_zero_disables_size_gate() {
    // `0` means "no size gate" — the function should still reject a
    // missing file but the metadata branch is bypassed.
    let result = validate_lsp_file(Path::new("/nonexistent/file.rs"), 0);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("does not exist"));
}

#[test]
fn test_validate_lsp_file_too_large() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("big.rs");
    let mut f = std::fs::File::create(&path).expect("create");
    f.write_all(&vec![b'x'; 2048]).expect("write");
    let result = validate_lsp_file(&path, 1024);
    let err = result.expect_err("expected too-large rejection");
    assert!(
        err.contains("exceeds") && err.contains("limit"),
        "unexpected size-gate message: {err}"
    );
}
