use super::*;
use lsp_types::Diagnostic;
use lsp_types::Position;
use lsp_types::Range;
use lsp_types::Url;

fn make_diagnostic(line: u32, message: &str, severity: DiagnosticSeverity) -> Diagnostic {
    Diagnostic {
        range: Range {
            start: Position { line, character: 0 },
            end: Position {
                line,
                character: 10,
            },
        },
        severity: Some(severity),
        message: message.to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_update_and_get() {
    let store = DiagnosticsStore::new();

    let params = PublishDiagnosticsParams {
        uri: Url::parse("file:///test/file.rs").unwrap(),
        diagnostics: vec![
            make_diagnostic(10, "unused variable", DiagnosticSeverity::WARNING),
            make_diagnostic(20, "type error", DiagnosticSeverity::ERROR),
        ],
        version: Some(1),
    };

    store.update(params).await;

    let path = PathBuf::from("/test/file.rs");
    let entries = store.get_file(&path).await;

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].line, 11); // 0-indexed to 1-indexed
    assert_eq!(entries[0].message, "unused variable");
}

#[tokio::test]
async fn test_format_for_system_reminder() {
    let entries = vec![
        DiagnosticEntry {
            file: PathBuf::from("/test/file.rs"),
            line: 10,
            character: 5,
            severity: DiagnosticSeverityLevel::Error,
            message: "type mismatch".to_string(),
            code: Some("E0308".to_string()),
            source: Some("rust-analyzer".to_string()),
        },
        DiagnosticEntry {
            file: PathBuf::from("/test/file.rs"),
            line: 20,
            character: 1,
            severity: DiagnosticSeverityLevel::Warning,
            message: "unused import".to_string(),
            code: None,
            source: None,
        },
    ];

    let output = DiagnosticsStore::format_for_system_reminder(&entries);

    assert!(output.contains("<new-diagnostics>"));
    assert!(output.contains("type mismatch"));
    assert!(output.contains("[E0308]"));
    assert!(output.contains("(rust-analyzer)"));
    assert!(output.contains("[error]"));
    assert!(output.contains("[warning]"));
    assert!(output.contains("</new-diagnostics>"));
}

#[test]
fn test_severity_from_lsp() {
    assert_eq!(
        DiagnosticSeverityLevel::from(Some(DiagnosticSeverity::ERROR)),
        DiagnosticSeverityLevel::Error
    );
    assert_eq!(
        DiagnosticSeverityLevel::from(Some(DiagnosticSeverity::WARNING)),
        DiagnosticSeverityLevel::Warning
    );
    assert_eq!(
        DiagnosticSeverityLevel::from(None),
        DiagnosticSeverityLevel::Error
    );
}
