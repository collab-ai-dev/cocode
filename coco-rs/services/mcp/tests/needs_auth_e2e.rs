//! End-to-end coverage of MCP connect-time auth detection (Layer A) against a
//! real wiremock HTTP server. Validates that a connect failure WITH reachable
//! OAuth discovery lands `NeedsAuth` (and is cached), while a connect failure
//! WITHOUT OAuth discovery stays `Failed` (retryable) — the guard that keeps a
//! transient network fault from being mis-surfaced as "needs auth".
//!
//! The full OAuth token-exchange round-trip (browser redirect → localhost
//! callback → code/token exchange) is intentionally NOT covered here: it
//! exercises the rmcp SDK's OAuth state machine (dynamic client registration,
//! PKCE, `state` CSRF) rather than coco code, and mocking it faithfully is
//! brittle. The coco-owned seams around it are covered by unit tests
//! (`client.test.rs`): the needs-auth state mapping, the reconnect-notifier
//! plumbing, and the `has_discovery_but_no_token` skip + XAA guard.

use coco_mcp::McpConnectionManager;
use coco_mcp::SendElicitation;
use coco_mcp::types::ConfigScope;
use coco_mcp::types::McpHttpConfig;
use coco_mcp::types::McpServerConfig;
use coco_mcp::types::ScopedMcpServerConfig;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn noop_elicitation() -> SendElicitation {
    Box::new(|_id, _req| {
        Box::pin(async move { Err(coco_mcp::RmcpClientError::generic("no elicitation in test")) })
    })
}

fn register_http(manager: &mut McpConnectionManager, name: &str, url: String) {
    manager.register_server(ScopedMcpServerConfig {
        name: name.into(),
        config: McpServerConfig::Http(McpHttpConfig {
            url,
            headers: Default::default(),
            headers_helper: None,
            oauth: None,
        }),
        scope: ConfigScope::User,
        plugin_source: None,
    });
}

/// 401 on the MCP endpoint so the connect/initialize handshake fails.
async fn mount_mcp_401(server: &MockServer) {
    Mock::given(path("/mcp"))
        .respond_with(ResponseTemplate::new(401))
        .mount(server)
        .await;
}

/// 200 OAuth discovery advertising both endpoints → probe returns NotLoggedIn.
/// The probed path mirrors `discovery_paths("/mcp")[0]`.
async fn mount_oauth_discovery(server: &MockServer) {
    let body = serde_json::json!({
        "authorization_endpoint": format!("{}/authorize", server.uri()),
        "token_endpoint": format!("{}/token", server.uri()),
    });
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn connect_401_with_oauth_discovery_lands_needs_auth_and_caches() {
    let server = MockServer::start().await;
    mount_mcp_401(&server).await;
    mount_oauth_discovery(&server).await;

    let home = tempfile::tempdir().unwrap();
    let mut manager = McpConnectionManager::new(home.path().to_path_buf());
    let url = format!("{}/mcp", server.uri());
    register_http(&mut manager, "remote", url.clone());

    // connect() still reports the handshake error, but the resulting state
    // reflects the auth-required classification, not a generic failure.
    let result = manager.connect("remote", noop_elicitation()).await;
    assert!(
        result.is_err(),
        "connect should surface the handshake error"
    );

    assert!(
        matches!(
            manager.get_state("remote").await,
            Some(coco_mcp::McpConnectionState::NeedsAuth { .. })
        ),
        "a connect 401 with reachable OAuth discovery must surface NeedsAuth"
    );
    assert!(
        manager.is_needs_auth_cached("remote").await,
        "the needs-auth verdict must be cached so the next cycle skips the probe"
    );
    assert_eq!(
        manager.auth_descriptor("remote"),
        Some(("http".to_string(), Some(url))),
        "the descriptor feeds the per-server authenticate tool's description"
    );
}

#[tokio::test]
async fn connect_failure_without_oauth_discovery_stays_failed() {
    let server = MockServer::start().await;
    mount_mcp_401(&server).await;
    // No discovery mock → the discovery GET 404s → probe returns Unsupported.

    let home = tempfile::tempdir().unwrap();
    let mut manager = McpConnectionManager::new(home.path().to_path_buf());
    register_http(&mut manager, "remote", format!("{}/mcp", server.uri()));

    let result = manager.connect("remote", noop_elicitation()).await;
    assert!(result.is_err());

    assert!(
        matches!(
            manager.get_state("remote").await,
            Some(coco_mcp::McpConnectionState::Failed { .. })
        ),
        "a connect failure without OAuth discovery must stay Failed (retryable)"
    );
    assert!(
        !manager.is_needs_auth_cached("remote").await,
        "a genuine failure must NOT be cached as needs-auth"
    );
}
