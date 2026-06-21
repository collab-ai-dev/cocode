use super::*;

#[test]
fn test_find_by_extension() {
    let server = BuiltinServer::find_by_extension(".rs");
    assert!(server.is_some());
    assert_eq!(server.unwrap().id, "rust-analyzer");

    let server = BuiltinServer::find_by_extension(".go");
    assert!(server.is_some());
    assert_eq!(server.unwrap().id, "gopls");

    let server = BuiltinServer::find_by_extension(".py");
    assert!(server.is_some());
    assert_eq!(server.unwrap().id, "pyright");

    let server = BuiltinServer::find_by_extension(".ts");
    assert!(server.is_some());
    assert_eq!(server.unwrap().id, "typescript-language-server");

    let server = BuiltinServer::find_by_extension(".tsx");
    assert!(server.is_some());
    assert_eq!(server.unwrap().id, "typescript-language-server");

    let server = BuiltinServer::find_by_extension(".js");
    assert!(server.is_some());
    assert_eq!(server.unwrap().id, "typescript-language-server");

    let server = BuiltinServer::find_by_extension(".txt");
    assert!(server.is_none());
}

#[test]
fn test_find_by_id() {
    let server = BuiltinServer::find_by_id("rust-analyzer");
    assert!(server.is_some());

    let server = BuiltinServer::find_by_id("unknown");
    assert!(server.is_none());
}

#[test]
fn test_server_config_default() {
    let config = LspServerConfig::default();
    assert!(!config.disabled);
    assert!(config.command.is_none());
    assert!(config.args.is_empty());
    assert!(config.file_extensions.is_empty());
    assert_eq!(config.max_restarts, 3);
    assert!(config.restart_on_crash);
}

#[test]
fn test_server_config_is_custom() {
    let builtin_override = LspServerConfig {
        disabled: false,
        command: None,
        max_restarts: 5,
        ..Default::default()
    };
    assert!(!builtin_override.is_custom());

    let custom = LspServerConfig {
        command: Some("my-lsp".to_string()),
        args: vec!["--stdio".to_string()],
        file_extensions: vec![".xyz".to_string()],
        ..Default::default()
    };
    assert!(custom.is_custom());
}

#[test]
fn test_server_config_serde() {
    let json = r#"{
        "disabled": false,
        "command": "typescript-language-server",
        "args": ["--stdio"],
        "file_extensions": [".ts", ".tsx"],
        "languages": ["typescript"],
        "max_restarts": 5,
        "startup_timeout_ms": 15000
    }"#;

    let config: LspServerConfig = serde_json::from_str(json).unwrap();
    assert!(!config.disabled);
    assert_eq!(
        config.command,
        Some("typescript-language-server".to_string())
    );
    assert_eq!(config.args, vec!["--stdio"]);
    assert_eq!(config.file_extensions, vec![".ts", ".tsx"]);
    assert_eq!(config.languages, vec!["typescript"]);
    assert_eq!(config.max_restarts, 5);
    assert_eq!(config.startup_timeout_ms, 15_000);
}

#[test]
fn test_servers_config_serde() {
    let json = r#"{
        "servers": {
            "rust-analyzer": {
                "initialization_options": {"checkOnSave": {"command": "clippy"}},
                "max_restarts": 5
            },
            "gopls": {
                "disabled": true
            },
            "typescript": {
                "command": "typescript-language-server",
                "args": ["--stdio"],
                "file_extensions": [".ts", ".tsx", ".js", ".jsx"],
                "languages": ["typescript", "javascript"]
            }
        }
    }"#;

    let config: LspServersConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.servers.len(), 3);

    // Check rust-analyzer (builtin override)
    let ra = config.get("rust-analyzer").unwrap();
    assert!(!ra.is_custom());
    assert_eq!(ra.max_restarts, 5);

    // Check gopls (disabled)
    assert!(config.is_disabled("gopls"));

    // Check typescript (custom)
    let ts = config.get("typescript").unwrap();
    assert!(ts.is_custom());
    assert_eq!(ts.command, Some("typescript-language-server".to_string()));
}

#[test]
fn test_servers_config_merge() {
    let mut base = LspServersConfig::default();
    base.servers.insert(
        "rust-analyzer".to_string(),
        LspServerConfig {
            max_restarts: 3,
            ..Default::default()
        },
    );

    let override_config = LspServersConfig {
        servers: HashMap::from([(
            "rust-analyzer".to_string(),
            LspServerConfig {
                max_restarts: 10,
                ..Default::default()
            },
        )]),
    };

    base.merge(override_config);
    assert_eq!(base.get("rust-analyzer").unwrap().max_restarts, 10);
}

#[test]
fn test_lifecycle_config_from_server_config() {
    let server_config = LspServerConfig {
        max_restarts: 5,
        restart_on_crash: false,
        health_check_interval_ms: 60_000,
        startup_timeout_ms: 15_000,
        shutdown_timeout_ms: 3_000,
        request_timeout_ms: 45_000,
        ..Default::default()
    };
    let lifecycle: LifecycleConfig = (&server_config).into();
    assert_eq!(lifecycle.max_restarts, 5);
    assert!(!lifecycle.restart_on_crash);
    assert_eq!(lifecycle.health_check_interval_ms, 60_000);
}

#[test]
fn test_from_json_file() {
    use std::io::Write;

    let temp_dir = std::env::temp_dir().join("lsp_config_test_simplified");
    std::fs::create_dir_all(&temp_dir).unwrap();
    let json_path = temp_dir.join("lsp_servers.json");

    let json_content = r#"{
        "servers": {
            "clangd": {
                "command": "clangd",
                "args": ["--background-index"],
                "file_extensions": [".c", ".cpp", ".h"],
                "languages": ["c", "cpp"]
            }
        }
    }"#;

    let mut file = std::fs::File::create(&json_path).unwrap();
    file.write_all(json_content.as_bytes()).unwrap();

    let config = LspServersConfig::from_file(&json_path).unwrap();
    assert!(config.servers.contains_key("clangd"));
    let clangd = config.get("clangd").unwrap();
    assert!(clangd.is_custom());
    assert_eq!(clangd.command, Some("clangd".to_string()));

    // Cleanup
    std::fs::remove_file(&json_path).unwrap();
    let _ = std::fs::remove_dir(&temp_dir);
}

#[test]
fn test_from_file_not_found() {
    let result = LspServersConfig::from_file(Path::new("/nonexistent/path.json"));
    assert!(result.is_err());
}

#[test]
fn test_backward_compat_without_new_fields() {
    // Old config without new fields should still work
    let json = r#"{"disabled": false}"#;
    let config: LspServerConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.max_restarts, 3); // default
    assert!(config.restart_on_crash); // default
    assert_eq!(config.startup_timeout_ms, 10_000); // default
    assert_eq!(config.shutdown_timeout_ms, 5_000); // default
    assert_eq!(config.health_check_interval_ms, 30_000); // default
}
