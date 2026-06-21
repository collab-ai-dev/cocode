use super::*;

#[tokio::test]
async fn handler_with_no_args_renders_status_header() {
    let out = handler(String::new()).await.expect("handler ok");
    assert!(out.starts_with("## LSP servers"), "unexpected: {out}");
    // The "Available builtins" block enumerates every BUILTIN_SERVERS
    // entry — verify rust-analyzer surfaces so a future refactor that
    // accidentally hides them surfaces in CI.
    assert!(
        out.contains("rust-analyzer"),
        "missing rust-analyzer in: {out}"
    );
}

#[tokio::test]
async fn handler_install_unknown_id_is_caught_before_running_installer() {
    let out = handler("install nonexistent-lsp".to_string())
        .await
        .expect("handler ok");
    assert!(
        out.contains("Unknown builtin"),
        "expected unknown-builtin rejection, got: {out}"
    );
}

#[tokio::test]
async fn handler_with_args_but_no_id_shows_usage() {
    let out = handler("install".to_string()).await.expect("handler ok");
    assert!(out.starts_with("Usage:"), "expected usage, got: {out}");
}

#[tokio::test]
async fn handler_enable_for_unconfigured_id_reports_not_in_config() {
    // Use a unique throwaway id so the assertion is robust against
    // whatever the developer happens to have in their lsp_servers.json.
    let out = handler("enable __certainly_not_a_real_lsp_id__".to_string())
        .await
        .expect("handler ok");
    assert!(
        out.contains("not in any lsp_servers.json"),
        "expected not-in-config message, got: {out}"
    );
}
