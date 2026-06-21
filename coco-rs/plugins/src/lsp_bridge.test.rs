use std::path::Path;

use serde_json::json;

use crate::loader::LoadedPluginV2;
use crate::loader::PluginLoadSource;
use crate::schemas::PluginId;
use crate::schemas::PluginManifestV2;

use super::*;

fn test_plugin(name: &str, path: &Path, lsp_servers: Option<Value>) -> LoadedPluginV2 {
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
            mcp_servers: None,
            lsp_servers,
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
fn test_inline_object_with_extension_to_language() {
    let dir = tempfile::tempdir().unwrap();
    let lsp = json!({
        "my-lsp": {
            "command": "${CLAUDE_PLUGIN_ROOT}/bin/server",
            "args": ["--stdio"],
            "extensionToLanguage": { ".foo": "foolang" }
        }
    });
    let plugin = test_plugin("acme", dir.path(), Some(lsp));
    let servers = load_plugin_lsp_servers(&plugin);

    assert_eq!(servers.len(), 1);
    let config = servers.get("plugin:acme:my-lsp").expect("namespaced key");
    assert_eq!(config.file_extensions, vec![".foo".to_string()]);
    assert_eq!(config.languages, vec!["foolang".to_string()]);
    // ${CLAUDE_PLUGIN_ROOT} substituted with the plugin path.
    assert_eq!(
        config.command.as_deref(),
        Some(
            dir.path()
                .join("bin/server")
                .to_string_lossy()
                .to_string()
                .as_str()
        )
    );
    // COCO_PLUGIN_ROOT injected into env.
    assert_eq!(
        config.env.get("COCO_PLUGIN_ROOT").map(String::as_str),
        Some(dir.path().display().to_string().as_str())
    );
}

#[test]
fn test_native_field_shape() {
    let dir = tempfile::tempdir().unwrap();
    let lsp = json!({
        "native": {
            "command": "served",
            "file_extensions": [".rs"],
            "languages": ["rust"]
        }
    });
    let plugin = test_plugin("p", dir.path(), Some(lsp));
    let servers = load_plugin_lsp_servers(&plugin);
    let config = servers.get("plugin:p:native").expect("server");
    assert_eq!(config.file_extensions, vec![".rs".to_string()]);
    assert_eq!(config.languages, vec!["rust".to_string()]);
}

#[test]
fn test_dot_lsp_json_loaded() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".lsp.json"),
        json!({ "fromfile": { "command": "x", "file_extensions": [".x"] } }).to_string(),
    )
    .unwrap();
    let plugin = test_plugin("filep", dir.path(), None);
    let servers = load_plugin_lsp_servers(&plugin);
    assert!(servers.contains_key("plugin:filep:fromfile"));
}

#[test]
fn test_manifest_overrides_dot_lsp_json() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".lsp.json"),
        json!({ "srv": { "command": "from-file" } }).to_string(),
    )
    .unwrap();
    let lsp = json!({ "srv": { "command": "from-manifest" } });
    let plugin = test_plugin("o", dir.path(), Some(lsp));
    let servers = load_plugin_lsp_servers(&plugin);
    assert_eq!(
        servers.get("plugin:o:srv").unwrap().command.as_deref(),
        Some("from-manifest")
    );
}

#[test]
fn test_extract_merges_multiple_plugins() {
    let dir1 = tempfile::tempdir().unwrap();
    let dir2 = tempfile::tempdir().unwrap();
    let p1 = test_plugin("one", dir1.path(), Some(json!({ "a": { "command": "a" } })));
    let p2 = test_plugin("two", dir2.path(), Some(json!({ "b": { "command": "b" } })));
    let merged = extract_lsp_servers_from_plugins(&[&p1, &p2]);
    assert_eq!(merged.servers.len(), 2);
    assert!(merged.servers.contains_key("plugin:one:a"));
    assert!(merged.servers.contains_key("plugin:two:b"));
}

#[test]
fn test_path_traversal_rejected() {
    let dir = tempfile::tempdir().unwrap();
    // A string path escaping the plugin root must not load anything.
    let plugin = test_plugin("evil", dir.path(), Some(json!("../../etc/passwd")));
    let servers = load_plugin_lsp_servers(&plugin);
    assert!(servers.is_empty());
}
