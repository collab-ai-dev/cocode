use super::*;

fn noop_elicitation() -> SendElicitation {
    Box::new(|_id, _req| {
        Box::pin(async move {
            Err(coco_rmcp_client::RmcpClientError::generic(
                "not used by test",
            ))
        })
    })
}

#[test]
fn headers_helper_output_must_be_string_map() {
    let ok = parse_headers_helper_output("srv", r#"{"Authorization":"Bearer x"}"#).unwrap();
    assert_eq!(ok.get("Authorization").unwrap(), "Bearer x");

    let err = parse_headers_helper_output("srv", r#"{"Authorization":123}"#).unwrap_err();
    assert!(err.to_string().contains("non-string"));
}

#[test]
fn resource_directory_read_capability_reads_skills_extension() {
    let capabilities: ServerCapabilities = serde_json::from_value(serde_json::json!({
        "resources": {
            "extensions": {
                "io.modelcontextprotocol/skills": {
                    "directoryRead": true
                }
            }
        }
    }))
    .unwrap();

    assert!(server_supports_resource_directory_read(&capabilities));
}

#[test]
fn resource_directory_read_capability_accepts_top_level_compat_field() {
    let capabilities: ServerCapabilities = serde_json::from_value(serde_json::json!({
        "resources": {
            "directoryRead": true
        }
    }))
    .unwrap();

    assert!(server_supports_resource_directory_read(&capabilities));
}

#[tokio::test]
async fn resolve_http_headers_dynamic_overrides_static() {
    let headers = resolve_http_headers(
        "srv",
        "https://example.test",
        &HashMap::from([
            ("Authorization".to_string(), "Bearer old".to_string()),
            ("X-Static".to_string(), "yes".to_string()),
        ]),
        &Some("printf '{\"Authorization\":\"Bearer new\"}'".to_string()),
    )
    .await
    .unwrap();

    assert_eq!(headers.get("Authorization").unwrap(), "Bearer new");
    assert_eq!(headers.get("X-Static").unwrap(), "yes");
}

#[tokio::test]
async fn authenticate_stdio_reports_oauth_not_needed() {
    let mut manager = McpConnectionManager::new(std::env::temp_dir());
    manager.register_server(crate::types::ScopedMcpServerConfig {
        name: "local".into(),
        config: crate::types::McpServerConfig::Stdio(crate::types::McpStdioConfig {
            command: "echo".into(),
            args: vec![],
            env: Default::default(),
            cwd: None,
        }),
        scope: crate::types::ConfigScope::User,
        plugin_source: None,
    });

    let result = manager
        .authenticate("local", noop_elicitation())
        .await
        .unwrap();
    assert_eq!(
        result,
        "MCP server 'local' does not use OAuth authentication."
    );
}

#[tokio::test]
async fn unregister_server_drops_config_and_connection_state() {
    let mut manager = McpConnectionManager::new(std::env::temp_dir());
    manager.register_server(crate::types::ScopedMcpServerConfig {
        name: "plugin:p:local".into(),
        config: crate::types::McpServerConfig::Stdio(crate::types::McpStdioConfig {
            command: "echo".into(),
            args: vec![],
            env: Default::default(),
            cwd: None,
        }),
        scope: crate::types::ConfigScope::Dynamic,
        plugin_source: None,
    });
    // register_server seeds a Pending connection state + a config entry.
    assert!(
        manager
            .registered_server_names()
            .contains(&"plugin:p:local".to_string())
    );
    assert!(manager.get_state("plugin:p:local").await.is_some());

    manager.unregister_server("plugin:p:local").await;
    assert!(
        !manager
            .registered_server_names()
            .contains(&"plugin:p:local".to_string()),
        "config entry must be dropped"
    );
    assert!(
        manager.get_state("plugin:p:local").await.is_none(),
        "connection state must be dropped"
    );
}

#[tokio::test]
async fn ensure_xaa_tokens_skips_exchange_when_stored_tokens_exist() {
    let home = tempfile::tempdir().unwrap();
    coco_rmcp_client::save_oauth_access_token(coco_rmcp_client::OAuthAccessTokenSave {
        server_name: "enterprise",
        url: "https://mcp.example.test",
        client_id: "as-client",
        access_token: "stored-token".to_string(),
        refresh_token: None,
        expires_in: Some(3600),
        scopes: None,
        store_mode: OAuthCredentialsStoreMode::File,
        config_home: home.path(),
    })
    .unwrap();

    let oauth = McpOAuthConfig {
        client_id: Some("as-client".into()),
        xaa: Some(crate::types::McpXaaConfig {
            client_id: None,
            client_secret: Some("as-secret".into()),
            idp_client_id: Some("idp-client".into()),
            idp_client_secret: None,
            idp_id_token: None,
            idp_token_endpoint: Some("https://idp.example.test/token".into()),
            scope: None,
        }),
    };

    let result = ensure_xaa_tokens(
        "enterprise",
        "https://mcp.example.test",
        Some(&oauth),
        home.path(),
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn ensure_xaa_tokens_errors_on_missing_idp_token_without_stored_tokens() {
    let home = tempfile::tempdir().unwrap();
    let oauth = McpOAuthConfig {
        client_id: Some("as-client".into()),
        xaa: Some(crate::types::McpXaaConfig {
            client_id: None,
            client_secret: Some("as-secret".into()),
            idp_client_id: Some("idp-client".into()),
            idp_client_secret: None,
            idp_id_token: None,
            idp_token_endpoint: Some("https://idp.example.test/token".into()),
            scope: None,
        }),
    };

    let err = ensure_xaa_tokens(
        "enterprise-missing",
        "https://mcp-missing.example.test",
        Some(&oauth),
        home.path(),
    )
    .await
    .expect_err("missing idp token should fail before exchange");
    assert!(err.to_string().contains("oauth.xaa.idpIdToken"));
}

// ---------------------------------------------------------------------------
// auth_descriptor + needs_auth_without_connect — per-server auth surfacing
// ---------------------------------------------------------------------------

fn register_http(
    manager: &mut McpConnectionManager,
    name: &str,
    oauth: Option<crate::types::McpOAuthConfig>,
) {
    manager.register_server(crate::types::ScopedMcpServerConfig {
        name: name.into(),
        config: crate::types::McpServerConfig::Http(crate::types::McpHttpConfig {
            url: "https://mcp.example.test/api".into(),
            headers: Default::default(),
            headers_helper: None,
            oauth,
        }),
        scope: crate::types::ConfigScope::User,
        plugin_source: None,
    });
}

fn register_http_with_headers_helper(manager: &mut McpConnectionManager, name: &str) {
    manager.register_server(crate::types::ScopedMcpServerConfig {
        name: name.into(),
        config: crate::types::McpServerConfig::Http(crate::types::McpHttpConfig {
            url: "https://mcp.example.test/api".into(),
            headers: Default::default(),
            headers_helper: Some("printf '{}'".into()),
            oauth: None,
        }),
        scope: crate::types::ConfigScope::User,
        plugin_source: None,
    });
}

#[test]
fn tool_idle_timeout_is_remote_only_and_clamped_to_overall_timeout() {
    let runtime = coco_config::McpRuntimeConfig {
        tool_timeout_ms: Some(2_000),
        tool_idle_timeout_ms: Some(10_000),
    };
    let manager = McpConnectionManager::new_with_runtime_config(std::env::temp_dir(), &runtime);

    assert_eq!(manager.tool_timeout_ms(), 2_000);
    assert_eq!(manager.tool_idle_timeout_ms(), 10_000);

    let http = crate::types::McpServerConfig::Http(crate::types::McpHttpConfig {
        url: "https://mcp.example.test/api".into(),
        headers: Default::default(),
        headers_helper: None,
        oauth: None,
    });
    assert_eq!(
        manager.effective_tool_idle_timeout(&http),
        Some(std::time::Duration::from_millis(2_000))
    );

    let stdio = crate::types::McpServerConfig::Stdio(crate::types::McpStdioConfig {
        command: "echo".into(),
        args: vec![],
        env: Default::default(),
        cwd: None,
    });
    assert_eq!(manager.effective_tool_idle_timeout(&stdio), None);
}

#[test]
fn tool_idle_timeout_zero_disables_remote_idle_watchdog() {
    let runtime = coco_config::McpRuntimeConfig {
        tool_timeout_ms: Some(2_000),
        tool_idle_timeout_ms: Some(0),
    };
    let manager = McpConnectionManager::new_with_runtime_config(std::env::temp_dir(), &runtime);
    let http = crate::types::McpServerConfig::Http(crate::types::McpHttpConfig {
        url: "https://mcp.example.test/api".into(),
        headers: Default::default(),
        headers_helper: None,
        oauth: None,
    });

    assert_eq!(manager.tool_idle_timeout_ms(), 0);
    assert_eq!(manager.effective_tool_idle_timeout(&http), None);
}

#[test]
fn tool_call_auth_reconnect_requires_headers_helper_and_401_or_403() {
    let mut manager = McpConnectionManager::new(std::env::temp_dir());
    register_http_with_headers_helper(&mut manager, "remote");
    register_http(&mut manager, "plain", None);

    assert!(manager.should_reconnect_tool_call_auth_error(
        "remote",
        &McpClientError::ToolCallHttpStatus {
            status: 401,
            message: "unauthorized".into(),
        }
    ));
    assert!(manager.should_reconnect_tool_call_auth_error(
        "remote",
        &McpClientError::ToolCallHttpStatus {
            status: 403,
            message: "forbidden".into(),
        }
    ));
    assert!(!manager.should_reconnect_tool_call_auth_error(
        "remote",
        &McpClientError::ToolCallHttpStatus {
            status: 500,
            message: "server error".into(),
        }
    ));
    assert!(!manager.should_reconnect_tool_call_auth_error(
        "plain",
        &McpClientError::ToolCallHttpStatus {
            status: 401,
            message: "unauthorized".into(),
        }
    ));
}

#[tokio::test]
async fn discovery_retry_list_retries_transient_failure_once() {
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let result = retry_discovery_list("remote", "tools/list", || {
        let attempts = std::sync::Arc::clone(&attempts);
        async move {
            if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Err(RmcpClientError::generic("temporary network failure"))
            } else {
                Ok("ok")
            }
        }
    })
    .await
    .unwrap();

    assert_eq!(result, "ok");
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn http_endpoint_not_found_message_sanitizes_config_url() {
    assert_eq!(
        http_endpoint_not_found_message(
            "https://user:pass@mcp.example.test/api/?token=secret#frag",
        ),
        "MCP endpoint not found at https://mcp.example.test/api. Check the URL in your MCP config."
    );
}

#[test]
fn http_endpoint_not_found_message_handles_unparseable_url() {
    assert_eq!(
        http_endpoint_not_found_message("not a url"),
        "MCP endpoint not found at (unparseable url). Check the URL in your MCP config."
    );
}

#[test]
fn session_ingress_mcp_url_matches_only_known_paths_on_same_origin() {
    assert!(is_session_ingress_mcp_url_with_base(
        "https://api.anthropic.test/v2/session_ingress/shttp/mcp/server",
        Some("https://api.anthropic.test"),
    ));
    assert!(!is_session_ingress_mcp_url_with_base(
        "https://api.anthropic.test/custom/mcp",
        Some("https://api.anthropic.test"),
    ));
    assert!(!is_session_ingress_mcp_url_with_base(
        "https://other.anthropic.test/v2/session_ingress/shttp/mcp/server",
        Some("https://api.anthropic.test"),
    ));
}

#[test]
fn auth_descriptor_reports_http_transport_and_url() {
    let mut manager = McpConnectionManager::new(std::env::temp_dir());
    register_http(&mut manager, "remote", None);
    assert_eq!(
        manager.auth_descriptor("remote"),
        Some((
            "http".to_string(),
            Some("https://mcp.example.test/api".to_string())
        ))
    );
}

#[test]
fn auth_descriptor_reports_stdio_without_url() {
    let mut manager = McpConnectionManager::new(std::env::temp_dir());
    manager.register_server(crate::types::ScopedMcpServerConfig {
        name: "local".into(),
        config: crate::types::McpServerConfig::Stdio(crate::types::McpStdioConfig {
            command: "echo".into(),
            args: vec![],
            env: Default::default(),
            cwd: None,
        }),
        scope: crate::types::ConfigScope::User,
        plugin_source: None,
    });
    assert_eq!(
        manager.auth_descriptor("local"),
        Some(("stdio".to_string(), None))
    );
}

#[test]
fn auth_descriptor_none_for_unregistered_server() {
    let manager = McpConnectionManager::new(std::env::temp_dir());
    assert_eq!(manager.auth_descriptor("ghost"), None);
}

#[test]
fn needs_auth_without_connect_false_for_stdio() {
    let mut manager = McpConnectionManager::new(std::env::temp_dir());
    manager.register_server(crate::types::ScopedMcpServerConfig {
        name: "local".into(),
        config: crate::types::McpServerConfig::Stdio(crate::types::McpStdioConfig {
            command: "echo".into(),
            args: vec![],
            env: Default::default(),
            cwd: None,
        }),
        scope: crate::types::ConfigScope::User,
        plugin_source: None,
    });
    assert!(!manager.needs_auth_without_connect("local"));
}

#[test]
fn needs_auth_without_connect_false_for_xaa_server() {
    // XAA guard: an xaa-configured server can silently re-auth from a cached
    // IdP id_token, so it must NOT be skip-surfaced (else the silent re-auth
    // branch is unreachable).
    let mut manager = McpConnectionManager::new(std::env::temp_dir());
    register_http(
        &mut manager,
        "xaa-srv",
        Some(crate::types::McpOAuthConfig {
            client_id: None,
            xaa: Some(crate::types::McpXaaConfig {
                client_id: None,
                client_secret: None,
                idp_client_id: None,
                idp_client_secret: None,
                idp_id_token: None,
                idp_token_endpoint: None,
                scope: None,
            }),
        }),
    );
    assert!(!manager.needs_auth_without_connect("xaa-srv"));
}

#[test]
fn needs_auth_without_connect_false_when_no_discovery_entry() {
    // A plain OAuth-capable server with no stored token entry has no discovery
    // state yet, so we should still attempt the connect (returns false).
    let mut manager = McpConnectionManager::new(std::env::temp_dir());
    register_http(&mut manager, "fresh", None);
    assert!(!manager.needs_auth_without_connect("fresh"));
}

// ---------------------------------------------------------------------------
// has_discovery_but_no_token skip + XAA guard (discriminating cases) + notifier
// ---------------------------------------------------------------------------

/// URL hardcoded by `register_http` above — must match for the token store key.
const REGISTERED_HTTP_URL: &str = "https://mcp.example.test/api";

/// Seed the coco OAuth store with an entry that has no usable credentials
/// (empty access token, no refresh token) — the steady-state "discovery but no
/// token" condition `has_discovery_but_no_token` detects.
fn seed_empty_token(home: &std::path::Path, name: &str, url: &str) {
    let store = crate::auth::OAuthTokenStore::from_config_home(home);
    let key = crate::auth::server_key(name, url);
    store
        .save(
            &key,
            &crate::auth::OAuthTokens {
                access_token: String::new(),
                refresh_token: None,
                expires_at: None,
                token_type: String::new(),
            },
        )
        .unwrap();
}

fn xaa_oauth() -> crate::types::McpOAuthConfig {
    crate::types::McpOAuthConfig {
        client_id: None,
        xaa: Some(crate::types::McpXaaConfig {
            client_id: None,
            client_secret: None,
            idp_client_id: None,
            idp_client_secret: None,
            idp_id_token: None,
            idp_token_endpoint: None,
            scope: None,
        }),
    }
}

#[test]
fn needs_auth_without_connect_true_when_discovery_but_no_token() {
    let home = tempfile::tempdir().unwrap();
    let mut manager = McpConnectionManager::new(home.path().to_path_buf());
    register_http(&mut manager, "remote", None);
    seed_empty_token(home.path(), "remote", REGISTERED_HTTP_URL);
    assert!(
        manager.needs_auth_without_connect("remote"),
        "a non-XAA server with discovery state but no token would 401 → skip + surface auth tool"
    );
}

#[test]
fn needs_auth_without_connect_false_for_xaa_even_with_discovery_but_no_token() {
    // XAA guard: even with the exact discovery-but-no-token condition that
    // triggers a skip for a normal server, an XAA server must NOT be skipped —
    // it can silently re-auth from a cached IdP id_token, so we must still
    // attempt the connect (else the silent-reauth path is unreachable).
    let home = tempfile::tempdir().unwrap();
    let mut manager = McpConnectionManager::new(home.path().to_path_buf());
    register_http(&mut manager, "xaa-srv", Some(xaa_oauth()));
    seed_empty_token(home.path(), "xaa-srv", REGISTERED_HTTP_URL);
    assert!(
        !manager.needs_auth_without_connect("xaa-srv"),
        "XAA silent-reauth servers must still attempt the connect"
    );
}

#[tokio::test]
async fn reconnect_notifier_receives_server_after_notify() {
    // Layer C plumbing: a background reconnect notifies the app-layer listener
    // with the server name so it can re-reconcile the tool registry.
    let manager = McpConnectionManager::new(std::env::temp_dir());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    manager.set_reconnect_notifier(tx);
    manager.notify_reconnect("remote");
    assert_eq!(rx.recv().await, Some("remote".to_string()));
}

#[tokio::test]
async fn reconnect_notifier_is_noop_without_listener() {
    // No listener wired → notify must not panic (SDK / test paths).
    let manager = McpConnectionManager::new(std::env::temp_dir());
    manager.notify_reconnect("remote");
}
