use super::*;
use coco_system_reminder::DiagnosticsSource;
use coco_system_reminder::IdeBridgeSource;
use std::sync::Arc;

#[tokio::test]
async fn lsp_adapter_empty_store_returns_empty() {
    let store = Arc::new(coco_lsp::DiagnosticsStore::new());
    let adapter = LspDiagnosticsAdapter::new(store);
    assert!(adapter.snapshot(None).await.is_empty());
}

#[test]
fn lsp_formatter_groups_counts() {
    use coco_lsp::diagnostics::DiagnosticEntry;
    use coco_lsp::diagnostics::DiagnosticSeverityLevel;
    use std::path::PathBuf;

    let entries = vec![
        DiagnosticEntry {
            file: PathBuf::from("foo.rs"),
            line: 9,
            character: 4,
            severity: DiagnosticSeverityLevel::Error,
            message: "unknown ident".into(),
            code: None,
            source: None,
        },
        DiagnosticEntry {
            file: PathBuf::from("foo.rs"),
            line: 15,
            character: 0,
            severity: DiagnosticSeverityLevel::Warning,
            message: "unused var".into(),
            code: None,
            source: None,
        },
    ];
    let block = format_file_block(&PathBuf::from("foo.rs"), &entries);
    assert!(block.starts_with("foo.rs: 1 error, 1 warning"));
    assert!(block.contains("10:5 [error] unknown ident"));
    assert!(block.contains("16:1 [warning] unused var"));
}

#[tokio::test]
async fn ide_bridge_adapter_is_none() {
    let adapter = IdeBridgeAdapter::new();
    assert!(adapter.selection(None).await.is_none());
    assert!(adapter.opened_file(None).await.is_none());
}
