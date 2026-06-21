use std::path::Path;

use coco_mcp::McpServerConfig;
use serde_json::json;

use crate::loader::LoadedPluginV2;
use crate::loader::PluginLoadSource;
use crate::schemas::PluginId;
use crate::schemas::PluginManifestV2;

use super::*;

fn test_plugin(name: &str, path: &Path, mcp_servers: Option<Value>) -> LoadedPluginV2 {
    LoadedPluginV2 {
        id: PluginId {
            name: name.to_string(),
            marketplace: "inline".to_string(),
        },
        manifest: PluginManifestV2 {
            name: name.to_string(),
            version: None,
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: None,
            dependencies: None,
            skills: None,
            hooks: None,
            agents: None,
            commands: None,
            mcp_servers,
            lsp_servers: None,
            output_styles: None,
            channels: None,
            user_config: None,
            settings: None,
            env_vars: None,
            min_version: None,
            max_version: None,
        },
        path: path.to_path_buf(),
        load_source: PluginLoadSource::SessionDir,
        enabled: true,
    }
}

#[test]
fn test_inline_stdio_namespaced_and_env_injected() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = json!({
        "db": {
            "command": "${CLAUDE_PLUGIN_ROOT}/bin/db-server",
            "args": ["--port", "1234"]
        }
    });
    let plugin = test_plugin("acme", dir.path(), Some(mcp));
    let servers = load_plugin_mcp_servers(&plugin);

    assert_eq!(servers.len(), 1);
    let scoped = &servers[0];
    assert_eq!(scoped.name, "plugin:acme:db");
    assert_eq!(scoped.scope, coco_mcp::ConfigScope::Dynamic);
    assert_eq!(scoped.plugin_source.as_deref(), Some("acme@inline"));

    let McpServerConfig::Stdio(stdio) = &scoped.config else {
        panic!("expected stdio config");
    };
    // ${CLAUDE_PLUGIN_ROOT} substituted into the command.
    assert_eq!(
        stdio.command,
        dir.path().join("bin/db-server").to_string_lossy()
    );
    // Both plugin-root env vars injected.
    assert_eq!(
        stdio.env.get("COCO_PLUGIN_ROOT").map(String::as_str),
        Some(dir.path().display().to_string().as_str())
    );
    assert!(stdio.env.contains_key("CLAUDE_PLUGIN_ROOT"));
}

#[test]
fn test_http_server_parsed() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = json!({
        "remote": { "url": "https://example.com/mcp", "transport": "http" }
    });
    let plugin = test_plugin("p", dir.path(), Some(mcp));
    let servers = load_plugin_mcp_servers(&plugin);
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "plugin:p:remote");
    assert!(matches!(servers[0].config, McpServerConfig::Http(_)));
}

#[test]
fn test_dot_mcp_json_with_wrapper_key() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".mcp.json"),
        json!({ "mcpServers": { "fromfile": { "command": "x" } } }).to_string(),
    )
    .unwrap();
    let plugin = test_plugin("filep", dir.path(), None);
    let servers = load_plugin_mcp_servers(&plugin);
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "plugin:filep:fromfile");
}

#[test]
fn test_manifest_overrides_dot_mcp_json() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".mcp.json"),
        json!({ "srv": { "command": "from-file" } }).to_string(),
    )
    .unwrap();
    let mcp = json!({ "srv": { "command": "from-manifest" } });
    let plugin = test_plugin("o", dir.path(), Some(mcp));
    let servers = load_plugin_mcp_servers(&plugin);
    assert_eq!(servers.len(), 1);
    let McpServerConfig::Stdio(stdio) = &servers[0].config else {
        panic!("expected stdio");
    };
    assert_eq!(stdio.command, "from-manifest");
}

#[test]
fn test_disabled_server_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = json!({ "off": { "command": "x", "disabled": true } });
    let plugin = test_plugin("d", dir.path(), Some(mcp));
    assert!(load_plugin_mcp_servers(&plugin).is_empty());
}

#[test]
fn test_extract_merges_multiple_plugins() {
    let dir1 = tempfile::tempdir().unwrap();
    let dir2 = tempfile::tempdir().unwrap();
    let p1 = test_plugin("one", dir1.path(), Some(json!({ "a": { "command": "a" } })));
    let p2 = test_plugin("two", dir2.path(), Some(json!({ "b": { "command": "b" } })));
    let merged = extract_mcp_servers_from_plugins(&[&p1, &p2]);
    assert_eq!(merged.len(), 2);
    let names: Vec<&str> = merged.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"plugin:one:a"));
    assert!(names.contains(&"plugin:two:b"));
}
