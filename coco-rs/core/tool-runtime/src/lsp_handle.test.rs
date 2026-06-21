use super::*;

#[tokio::test]
async fn no_op_handle_reports_not_connected() {
    let handle = NoOpLspHandle;
    assert!(!handle.is_connected());
}

#[tokio::test]
async fn no_op_send_request_returns_error() {
    let handle = NoOpLspHandle;
    let result = handle
        .send_request(
            std::path::Path::new("/tmp/foo.rs"),
            "textDocument/definition",
            serde_json::json!({}),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn no_op_notify_save_is_silent() {
    let handle = NoOpLspHandle;
    // Must not panic.
    handle
        .notify_save(std::path::Path::new("/tmp/foo.rs"))
        .await;
}
