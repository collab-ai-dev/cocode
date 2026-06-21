use super::*;
use coco_lsp::DiagnosticsStore;
use coco_lsp::LspServersConfig;
use coco_tool_runtime::LspHandle;
use std::sync::Arc;

#[tokio::test]
async fn adapter_reports_disconnected_when_no_servers_configured() {
    // Empty LspServersConfig => has_configured_servers() should be false.
    let diagnostics = Arc::new(DiagnosticsStore::new());
    let manager = Arc::new(LspServerManager::new(
        LspServersConfig::default(),
        None,
        None,
        diagnostics,
    ));
    let adapter = LspManagerAdapter::new(manager);
    assert!(!adapter.is_connected());
}

#[tokio::test]
async fn notify_save_is_silent_without_server() {
    let diagnostics = Arc::new(DiagnosticsStore::new());
    let manager = Arc::new(LspServerManager::new(
        LspServersConfig::default(),
        None,
        None,
        diagnostics,
    ));
    let adapter = LspManagerAdapter::new(manager);
    // Must not panic. The file doesn't exist and there's no server —
    // the adapter swallows both.
    adapter
        .notify_save(std::path::Path::new("/tmp/__does_not_exist__.rs"))
        .await;
}

#[tokio::test]
async fn prewarm_with_no_servers_keeps_disconnected() {
    // Empty config → no extensions → `prewarm` should leave `has_active`
    // at its initial value (false, because `has_configured_servers()`
    // is false). `is_connected` must remain false so `LspTool` stays
    // filtered out of the model's tool list.
    let diagnostics = Arc::new(DiagnosticsStore::new());
    let manager = Arc::new(LspServerManager::new(
        LspServersConfig::default(),
        None,
        None,
        diagnostics,
    ));
    let adapter = LspManagerAdapter::new(manager);
    let tmp = std::env::temp_dir();
    adapter.prewarm(&tmp).await;
    assert!(
        !adapter.is_connected(),
        "no config → no servers → not connected"
    );
}
