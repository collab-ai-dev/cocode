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

/// `parse_server_config` is pure shape detection: the removed `disabled`
/// field is not its concern (the loader's legacy fail-safe and
/// `crate::activation` own run/don't-run).
#[test]
fn test_parse_ignores_legacy_disabled_field() {
    let json = serde_json::json!({
        "command": "npx",
        "args": ["server"],
        "disabled": true
    });
    assert!(parse_server_config(&json).is_some());
    assert!(entry_is_legacy_disabled(&json));
    assert!(!entry_is_legacy_disabled(
        &serde_json::json!({"command": "npx"})
    ));
}

/// Fail-safe: a legacy `"disabled": true` entry never loads — and because the
/// merge is single and unconditional, a later-layer legacy entry *masks* an
/// earlier enabled definition instead of silently falling back to it.
#[test]
fn test_legacy_disabled_entry_refused_and_masks_earlier_layer() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("project");
    let config_home = tmp.path().join("config");
    std::fs::create_dir_all(&project_dir).unwrap();
    std::fs::create_dir_all(&config_home).unwrap();

    std::fs::write(
        config_home.join("mcp.json"),
        serde_json::json!({
            "mcpServers": { "srv": {"command": "user-cmd"} }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        config_home.join("managed-mcp.json"),
        serde_json::json!({
            "mcpServers": { "srv": {"command": "user-cmd", "disabled": true} }
        })
        .to_string(),
    )
    .unwrap();

    let loaded = McpConfigLoader::load(&project_dir, &config_home);
    assert!(
        loaded.is_empty(),
        "the managed entry wins the merge and fail-safes off; the user \
         definition must not survive underneath it"
    );
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

/// `config_paths` must be exactly the file set the loader reads: a server
/// written into any listed path loads, and the loader reads nothing else.
#[test]
fn test_config_paths_matches_files_load_with_roots_reads() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("repo");
    let config_home = tmp.path().join("config");
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::create_dir_all(&config_home).unwrap();
    let roots = McpConfigRoots {
        project_root: &project_root,
        session_cwd: &project_root,
    };

    let paths = config_paths(roots, &config_home);
    // One uniquely-named server per listed file.
    for (index, (path, _)) in paths.iter().enumerate() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            path,
            serde_json::json!({
                "mcpServers": { format!("server-{index}"): {"command": "cmd"} }
            })
            .to_string(),
        )
        .unwrap();
    }

    let loaded = McpConfigLoader::load_with_roots(roots, &config_home);

    let mut loaded_names: Vec<_> = loaded.iter().map(|c| c.name.clone()).collect();
    loaded_names.sort();
    let mut expected: Vec<_> = (0..paths.len()).map(|i| format!("server-{i}")).collect();
    expected.sort();
    assert_eq!(loaded_names, expected, "every config_paths file must load");
}

#[test]
fn test_config_paths_scopes_match_loaded_scopes() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("repo");
    let config_home = tmp.path().join("config");
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::create_dir_all(&config_home).unwrap();
    let roots = McpConfigRoots {
        project_root: &project_root,
        session_cwd: &project_root,
    };

    let paths = config_paths(roots, &config_home);
    for (index, (path, _)) in paths.iter().enumerate() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            path,
            serde_json::json!({
                "mcpServers": { format!("server-{index}"): {"command": "cmd"} }
            })
            .to_string(),
        )
        .unwrap();
    }

    let loaded = McpConfigLoader::load_with_roots(roots, &config_home);
    let by_name: std::collections::HashMap<_, _> = loaded
        .into_iter()
        .map(|config| (config.name.clone(), config))
        .collect();
    for (index, (_, scope)) in paths.iter().enumerate() {
        assert_eq!(
            by_name[&format!("server-{index}")].scope,
            *scope,
            "scope for config_paths entry {index} must match the loaded scope"
        );
    }
}

#[test]
fn test_defining_path_finds_legacy_disabled_entry_the_loader_refuses() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("repo");
    let config_home = tmp.path().join("config");
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::create_dir_all(&config_home).unwrap();
    let roots = McpConfigRoots {
        project_root: &project_root,
        session_cwd: &project_root,
    };

    std::fs::write(
        project_root.join(".mcp.json"),
        serde_json::json!({
            "mcpServers": { "off": {"command": "cmd", "disabled": true} }
        })
        .to_string(),
    )
    .unwrap();

    // Refused by the loader (fail-safe)...
    assert!(McpConfigLoader::load_with_roots(roots, &config_home).is_empty());
    // ...but still locatable, which is what `/mcp enable`'s migration needs.
    let (path, scope) = defining_path("off", roots, &config_home).unwrap();
    assert_eq!(path, project_root.join(".mcp.json"));
    assert_eq!(scope, ConfigScope::Project);
    let defined = defined_servers(roots, &config_home);
    assert!(defined[0].legacy_disabled);
    assert!(
        defined[0].config.is_some(),
        "shape still parses for display"
    );
}

#[test]
fn test_defining_path_returns_highest_precedence_definition() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("repo");
    let config_home = tmp.path().join("config");
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::create_dir_all(&config_home).unwrap();
    let roots = McpConfigRoots {
        project_root: &project_root,
        session_cwd: &project_root,
    };

    for path in [config_home.join("mcp.json"), project_root.join(".mcp.json")] {
        std::fs::write(
            path,
            serde_json::json!({ "mcpServers": { "dup": {"command": "cmd"} } }).to_string(),
        )
        .unwrap();
    }

    let (path, scope) = defining_path("dup", roots, &config_home).unwrap();
    assert_eq!(
        path,
        project_root.join(".mcp.json"),
        "project wins over user"
    );
    assert_eq!(scope, ConfigScope::Project);
    assert!(defining_path("absent", roots, &config_home).is_none());
}
