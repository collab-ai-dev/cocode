use super::*;

#[test]
fn test_parse_stdio_config() {
    let json = serde_json::json!({
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-filesystem"],
        "env": {"HOME": "/tmp"}
    });
    let config = parse_server_config(&json).unwrap();
    assert!(matches!(config, McpServerConfig::Stdio(_)));
    if let McpServerConfig::Stdio(stdio) = config {
        assert_eq!(stdio.command, "npx");
        assert_eq!(stdio.args.len(), 2);
        assert_eq!(stdio.env.get("HOME").unwrap(), "/tmp");
    }
}

#[test]
fn test_parse_stdio_with_cwd() {
    let json = serde_json::json!({
        "command": "node",
        "args": ["server.js"],
        "cwd": "/opt/mcp-server"
    });
    let config = parse_server_config(&json).unwrap();
    if let McpServerConfig::Stdio(stdio) = config {
        assert_eq!(stdio.cwd, Some(PathBuf::from("/opt/mcp-server")));
    }
}

#[test]
fn test_parse_sse_config() {
    let json = serde_json::json!({
        "url": "https://mcp.example.com/sse",
        "headers": {"Authorization": "Bearer token"}
    });
    let config = parse_server_config(&json).unwrap();
    assert!(matches!(config, McpServerConfig::Sse(_)));
}

#[test]
fn test_parse_http_config() {
    let json = serde_json::json!({
        "url": "https://mcp.example.com/api",
        "transport": "http",
        "headers": {"X-Api-Key": "key123"}
    });
    let config = parse_server_config(&json).unwrap();
    assert!(matches!(config, McpServerConfig::Http(_)));
    if let McpServerConfig::Http(http) = config {
        assert_eq!(http.url, "https://mcp.example.com/api");
        assert_eq!(http.headers.get("X-Api-Key").unwrap(), "key123");
    }
}

#[test]
fn test_parse_http_config_headers_helper() {
    let json = serde_json::json!({
        "url": "https://mcp.example.com/api",
        "transport": "http",
        "headers": {"X-Static": "a"},
        "headersHelper": "echo '{\"Authorization\":\"Bearer token\"}'"
    });
    let config = parse_server_config(&json).unwrap();
    let McpServerConfig::Http(http) = config else {
        panic!("expected http config");
    };
    assert_eq!(http.headers.get("X-Static").unwrap(), "a");
    assert_eq!(
        http.headers_helper.as_deref(),
        Some("echo '{\"Authorization\":\"Bearer token\"}'")
    );
}

#[test]
fn test_parse_http_xaa_oauth_config() {
    let json = serde_json::json!({
        "url": "https://mcp.example.com/api",
        "transport": "http",
        "oauth": {
            "clientId": "as-client",
            "xaa": {
                "clientSecret": "as-secret",
                "idpClientId": "idp-client",
                "idpClientSecret": "idp-secret",
                "idpIdToken": "id-token",
                "idpTokenEndpoint": "https://idp.example.com/token",
                "scope": "read write"
            }
        }
    });
    let config = parse_server_config(&json).unwrap();
    let McpServerConfig::Http(http) = config else {
        panic!("expected http config");
    };
    let oauth = http.oauth.expect("oauth config");
    assert_eq!(oauth.client_id.as_deref(), Some("as-client"));
    let xaa = oauth.xaa.expect("xaa config");
    assert_eq!(xaa.client_secret.as_deref(), Some("as-secret"));
    assert_eq!(xaa.idp_client_id.as_deref(), Some("idp-client"));
    assert_eq!(xaa.idp_client_secret.as_deref(), Some("idp-secret"));
    assert_eq!(xaa.idp_id_token.as_deref(), Some("id-token"));
    assert_eq!(
        xaa.idp_token_endpoint.as_deref(),
        Some("https://idp.example.com/token")
    );
    assert_eq!(xaa.scope.as_deref(), Some("read write"));
}

#[test]
fn test_parse_invalid_config() {
    let json = serde_json::json!({"invalid": true});
    assert!(parse_server_config(&json).is_none());
}

#[test]
fn test_parse_disabled_server_returns_none() {
    let json = serde_json::json!({
        "command": "npx",
        "args": ["server"],
        "disabled": true
    });
    assert!(parse_server_config(&json).is_none());
}

#[test]
fn test_load_deduplicates_by_name() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("project");
    std::fs::create_dir_all(&project_dir).unwrap();

    // Write project .mcp.json
    std::fs::write(
        project_dir.join(".mcp.json"),
        serde_json::json!({
            "mcpServers": {
                "server1": {"command": "project-server", "args": []}
            }
        })
        .to_string(),
    )
    .unwrap();

    // Write user mcp.json (in config_home)
    let config_home = tmp.path().join("config");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::write(
        config_home.join("mcp.json"),
        serde_json::json!({
            "mcpServers": {
                "server1": {"command": "user-server", "args": []}
            }
        })
        .to_string(),
    )
    .unwrap();

    let configs = McpConfigLoader::load(&project_dir, &config_home);
    // Project scope loads after user, so the project definition wins a name
    // collision.
    assert_eq!(configs.len(), 1);
    let server = &configs[0];
    assert_eq!(server.name, "server1");
    assert_eq!(server.scope, ConfigScope::Project);
    if let McpServerConfig::Stdio(stdio) = &server.config {
        assert_eq!(stdio.command, "project-server");
    }
}

#[test]
fn test_policy_scopes_cannot_be_name_shadowed() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("project");
    std::fs::create_dir_all(&project_dir).unwrap();
    let config_home = tmp.path().join("config");
    std::fs::create_dir_all(&config_home).unwrap();

    // Managed (policy-pushed) and enterprise definitions.
    std::fs::write(
        config_home.join("managed-mcp.json"),
        serde_json::json!({
            "mcpServers": { "managed_srv": {"command": "managed-cmd", "args": []} }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        config_home.join("enterprise-mcp.json"),
        serde_json::json!({
            "mcpServers": { "ent_srv": {"command": "enterprise-cmd", "args": []} }
        })
        .to_string(),
    )
    .unwrap();

    // A cloned repo's .mcp.json tries to shadow both policy servers by name.
    std::fs::write(
        project_dir.join(".mcp.json"),
        serde_json::json!({
            "mcpServers": {
                "managed_srv": {"command": "project-cmd", "args": []},
                "ent_srv": {"command": "project-cmd", "args": []}
            }
        })
        .to_string(),
    )
    .unwrap();

    let by_name: std::collections::HashMap<_, _> =
        McpConfigLoader::load(&project_dir, &config_home)
            .into_iter()
            .map(|config| (config.name.clone(), config))
            .collect();

    // Policy scopes load last, so project definitions cannot shadow them.
    assert_eq!(by_name["managed_srv"].scope, ConfigScope::Managed);
    assert_eq!(by_name["ent_srv"].scope, ConfigScope::Enterprise);
    let McpServerConfig::Stdio(managed) = &by_name["managed_srv"].config else {
        panic!("expected stdio config");
    };
    assert_eq!(
        managed.command, "managed-cmd",
        "project .mcp.json must not shadow a managed server"
    );
    let McpServerConfig::Stdio(enterprise) = &by_name["ent_srv"].config else {
        panic!("expected stdio config");
    };
    assert_eq!(
        enterprise.command, "enterprise-cmd",
        "project .mcp.json must not shadow an enterprise server"
    );
}

#[test]
fn test_load_with_roots_splits_project_and_local_roots() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("repo");
    let session_cwd = project_root.join("nested");
    let config_home = tmp.path().join("config");
    std::fs::create_dir_all(&session_cwd).unwrap();
    std::fs::create_dir_all(&config_home).unwrap();

    std::fs::write(
        project_root.join(".mcp.json"),
        serde_json::json!({
            "mcpServers": {
                "project": {"command": "project-server", "args": []},
                "shared": {"command": "project-shared", "args": []}
            }
        })
        .to_string(),
    )
    .unwrap();

    let local_dir = session_cwd.join(format!("{}.local", coco_utils_common::COCO_CONFIG_DIR_NAME));
    std::fs::create_dir_all(&local_dir).unwrap();
    std::fs::write(
        local_dir.join("mcp.json"),
        serde_json::json!({
            "mcpServers": {
                "local": {"command": "local-server", "args": []},
                "shared": {"command": "local-shared", "args": []}
            }
        })
        .to_string(),
    )
    .unwrap();

    let configs = McpConfigLoader::load_with_roots(
        McpConfigRoots {
            project_root: &project_root,
            session_cwd: &session_cwd,
        },
        &config_home,
    );

    let by_name: std::collections::HashMap<_, _> = configs
        .into_iter()
        .map(|config| (config.name.clone(), config))
        .collect();
    assert_eq!(by_name["project"].scope, ConfigScope::Project);
    assert_eq!(by_name["local"].scope, ConfigScope::Local);
    assert_eq!(by_name["shared"].scope, ConfigScope::Local);
    let McpServerConfig::Stdio(stdio) = &by_name["shared"].config else {
        panic!("expected stdio config");
    };
    assert_eq!(stdio.command, "local-shared");
}
