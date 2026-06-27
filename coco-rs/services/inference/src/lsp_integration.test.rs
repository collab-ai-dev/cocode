use super::*;

#[test]
fn test_format_diagnostics() {
    let diags = vec![Diagnostic {
        file_path: "src/main.rs".into(),
        line: 10,
        column: 5,
        severity: DiagnosticSeverity::Error,
        message: "unused variable".into(),
        source: Some("rustc".into()),
    }];
    let output = format_diagnostics(&diags);
    assert!(output.contains("src/main.rs:10:5"));
    assert!(output.contains("unused variable"));
}

#[test]
fn test_format_empty_diagnostics() {
    assert_eq!(format_diagnostics(&[]), "No diagnostics.");
}
