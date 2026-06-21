use super::*;

#[test]
fn test_supported_extensions() {
    let exts = LspServerManager::supported_extensions();
    assert!(exts.contains(&".rs"));
    assert!(exts.contains(&".go"));
    assert!(exts.contains(&".py"));
}

#[tokio::test]
async fn test_find_server_for_extension_opt_in() {
    // With no config, no servers should be available (opt-in design)
    let empty_config = LspServersConfig::default();
    let diagnostics = Arc::new(DiagnosticsStore::new());
    let manager = LspServerManager::new(empty_config, None, None, diagnostics);

    // No config = no servers
    assert!(manager.find_server_for_extension(".rs").await.is_err());
    assert!(manager.find_server_for_extension(".go").await.is_err());
    assert!(manager.find_server_for_extension(".txt").await.is_err());

    // With config, servers should be available
    let mut config = LspServersConfig::default();
    config.servers.insert(
        "rust-analyzer".to_string(),
        LspServerConfig::default(), // Uses builtin template
    );
    config.servers.insert(
        "gopls".to_string(),
        LspServerConfig::default(), // Uses builtin template
    );

    let diagnostics = Arc::new(DiagnosticsStore::new());
    let manager = LspServerManager::new(config, None, None, diagnostics);

    assert!(manager.find_server_for_extension(".rs").await.is_ok());
    assert!(manager.find_server_for_extension(".go").await.is_ok());
    assert!(manager.find_server_for_extension(".txt").await.is_err());
}

#[tokio::test]
async fn test_find_server_disabled() {
    let mut config = LspServersConfig::default();
    // Add rust-analyzer (disabled) and gopls (enabled)
    config.servers.insert(
        "rust-analyzer".to_string(),
        LspServerConfig {
            disabled: true,
            ..Default::default()
        },
    );
    config.servers.insert(
        "gopls".to_string(),
        LspServerConfig::default(), // Uses builtin template
    );

    let diagnostics = Arc::new(DiagnosticsStore::new());
    let manager = LspServerManager::new(config, None, None, diagnostics);

    // rust-analyzer is disabled
    assert!(manager.find_server_for_extension(".rs").await.is_err());
    // gopls is enabled
    assert!(manager.find_server_for_extension(".go").await.is_ok());
}

#[test]
fn test_find_project_root() {
    let config = LspServersConfig::default();
    let diagnostics = Arc::new(DiagnosticsStore::new());
    let manager = LspServerManager::new(config, None, None, diagnostics);

    // For non-existent paths, should return parent directory
    let root = manager.find_project_root(Path::new("/some/path/file.rs"));
    assert_eq!(root, Path::new("/some/path"));
}

#[tokio::test]
async fn test_custom_server_priority() {
    let mut config = LspServersConfig::default();

    // Add custom server for .rs extension (should override builtin)
    config.servers.insert(
        "my-rust-lsp".to_string(),
        LspServerConfig {
            command: Some("my-rust-lsp".to_string()),
            args: vec!["--stdio".to_string()],
            languages: vec!["rust".to_string()],
            file_extensions: vec![".rs".to_string()],
            ..Default::default()
        },
    );

    let diagnostics = Arc::new(DiagnosticsStore::new());
    let manager = LspServerManager::new(config, None, None, diagnostics);

    // Custom server should be found first
    let server_info = manager.find_server_for_extension(".rs").await.unwrap();
    assert_eq!(server_info.id, "my-rust-lsp");
    assert_eq!(server_info.command, "my-rust-lsp");
}

#[tokio::test]
async fn test_custom_server_new_extension() {
    let mut config = LspServersConfig::default();

    // Add custom server for a new extension
    config.servers.insert(
        "typescript-lsp".to_string(),
        LspServerConfig {
            command: Some("typescript-language-server".to_string()),
            args: vec!["--stdio".to_string()],
            languages: vec!["typescript".to_string()],
            file_extensions: vec![".ts".to_string(), ".tsx".to_string()],
            ..Default::default()
        },
    );

    let diagnostics = Arc::new(DiagnosticsStore::new());
    let manager = LspServerManager::new(config, None, None, diagnostics);

    // Custom extension should be found
    let server_info = manager.find_server_for_extension(".ts").await.unwrap();
    assert_eq!(server_info.id, "typescript-lsp");

    let server_info = manager.find_server_for_extension(".tsx").await.unwrap();
    assert_eq!(server_info.id, "typescript-lsp");
}

#[tokio::test]
async fn test_all_supported_extensions() {
    let mut config = LspServersConfig::default();

    // Add builtin reference (uses template for extensions)
    config
        .servers
        .insert("rust-analyzer".to_string(), LspServerConfig::default());

    // Add custom server with explicit extensions
    config.servers.insert(
        "typescript-lsp".to_string(),
        LspServerConfig {
            command: Some("tsc".to_string()),
            args: vec![],
            languages: vec!["typescript".to_string()],
            file_extensions: vec![".ts".to_string()],
            ..Default::default()
        },
    );

    let diagnostics = Arc::new(DiagnosticsStore::new());
    let manager = LspServerManager::new(config, None, None, diagnostics);

    let exts = manager.all_supported_extensions().await;
    // Only configured servers should be included
    assert!(exts.contains(&".rs".to_string())); // From rust-analyzer builtin template
    assert!(exts.contains(&".ts".to_string())); // From custom typescript-lsp
    // These are NOT included (not configured)
    assert!(!exts.contains(&".go".to_string()));
    assert!(!exts.contains(&".py".to_string()));
}

#[tokio::test]
async fn test_server_info_lifecycle_config() {
    let mut config = LspServersConfig::default();

    config.servers.insert(
        "rust-analyzer".to_string(),
        LspServerConfig {
            max_restarts: 5,
            restart_on_crash: false,
            startup_timeout_ms: 20_000,
            ..Default::default()
        },
    );

    let diagnostics = Arc::new(DiagnosticsStore::new());
    let manager = LspServerManager::new(config, None, None, diagnostics);

    let server_info = manager.find_server_for_extension(".rs").await.unwrap();
    assert_eq!(server_info.lifecycle_config.max_restarts, 5);
    assert!(!server_info.lifecycle_config.restart_on_crash);
    assert_eq!(server_info.lifecycle_config.startup_timeout_ms, 20_000);
}
