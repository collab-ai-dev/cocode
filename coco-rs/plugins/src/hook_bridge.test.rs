use std::collections::HashMap;

use coco_hooks::HookRegistry;
use coco_types::HookEventType;
use coco_types::HookScope;
use pretty_assertions::assert_eq;

use crate::loader::LoadedPluginV2;
use crate::loader::PluginLoadSource;
use crate::schemas::ManifestHooks;
use crate::schemas::PluginId;
use crate::schemas::PluginManifestV2;

use super::*;

/// Build a minimal inline `LoadedPluginV2` with optional inline manifest hooks.
fn test_plugin_with_hooks(
    name: &str,
    path: &std::path::Path,
    hooks: Option<ManifestHooks>,
) -> LoadedPluginV2 {
    LoadedPluginV2 {
        id: PluginId {
            name: name.to_string(),
            marketplace: "inline".to_string(),
        },
        manifest: PluginManifestV2 {
            name: name.to_string(),
            version: None,
            description: Some("Test plugin".to_string()),
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: None,
            dependencies: None,
            skills: None,
            hooks,
            agents: None,
            commands: None,
            mcp_servers: None,
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

fn test_plugin(name: &str, path: &std::path::Path) -> LoadedPluginV2 {
    test_plugin_with_hooks(name, path, None)
}

#[test]
fn test_load_hooks_from_hooks_dir() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join("hooks");
    std::fs::create_dir(&hooks_dir).unwrap();
    let hooks_json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [
            { "type": "command", "command": "echo pre", "matcher": "Bash" }
        ]
    });
    std::fs::write(
        hooks_dir.join("hooks.json"),
        serde_json::to_vec(&hooks_json).unwrap(),
    )
    .unwrap();

    let plugin = test_plugin("my-plugin", dir.path());
    let hooks = load_plugin_hooks_v2(&plugin);

    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].event, HookEventType::PreToolUse);
    assert_eq!(hooks[0].matcher.as_deref(), Some("Bash"));
    assert_eq!(hooks[0].scope, HookScope::Builtin);
    assert_eq!(
        hooks[0].status_message.as_deref(),
        Some("[my-plugin] running hook")
    );
}

#[test]
fn test_load_hooks_from_manifest_inline() {
    let dir = tempfile::tempdir().unwrap();
    let mut manifest_hooks = HashMap::new();
    manifest_hooks.insert(
        HookEventType::SessionStart.as_str().to_string(),
        serde_json::json!([{ "type": "prompt", "prompt": "hello from plugin" }]),
    );

    let plugin = test_plugin_with_hooks(
        "inline-plugin",
        dir.path(),
        Some(ManifestHooks::Inline(manifest_hooks)),
    );

    let hooks = load_plugin_hooks_v2(&plugin);

    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].event, HookEventType::SessionStart);
    assert_eq!(hooks[0].scope, HookScope::Builtin);
    assert!(
        hooks[0]
            .status_message
            .as_deref()
            .unwrap()
            .starts_with("[inline-plugin]")
    );
}

#[test]
fn test_load_hooks_deduplication() {
    let dir = tempfile::tempdir().unwrap();

    // hooks/hooks.json has the same hook as the manifest
    let hooks_dir = dir.path().join("hooks");
    std::fs::create_dir(&hooks_dir).unwrap();
    let make_def = || serde_json::json!([{ "type": "command", "command": "echo dup" }]);
    let hooks_json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): make_def()
    });
    std::fs::write(
        hooks_dir.join("hooks.json"),
        serde_json::to_vec(&hooks_json).unwrap(),
    )
    .unwrap();

    let mut manifest_hooks = HashMap::new();
    manifest_hooks.insert(HookEventType::PreToolUse.as_str().to_string(), make_def());

    let plugin = test_plugin_with_hooks(
        "dup-plugin",
        dir.path(),
        Some(ManifestHooks::Inline(manifest_hooks)),
    );

    // load_plugin_hooks_v2 returns both (no dedup at load time)
    let hooks = load_plugin_hooks_v2(&plugin);
    assert_eq!(hooks.len(), 2);

    // register_plugin_hooks_v2 deduplicates via register_deduped
    let registry = HookRegistry::new();
    register_plugin_hooks_v2(&registry, &[&plugin]);
    // The two hooks have the same command, so one is deduplicated
    assert_eq!(registry.len(), 1);
}

#[test]
fn test_load_hooks_empty_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = test_plugin("empty-plugin", dir.path());
    let hooks = load_plugin_hooks_v2(&plugin);
    assert!(hooks.is_empty());
}

#[test]
fn test_load_all_plugin_hooks_multiple_plugins() {
    let dir1 = tempfile::tempdir().unwrap();
    let dir2 = tempfile::tempdir().unwrap();

    let hooks_dir1 = dir1.path().join("hooks");
    std::fs::create_dir(&hooks_dir1).unwrap();
    let p1_json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [
            { "type": "command", "command": "echo p1" }
        ]
    });
    std::fs::write(
        hooks_dir1.join("hooks.json"),
        serde_json::to_vec(&p1_json).unwrap(),
    )
    .unwrap();

    let hooks_dir2 = dir2.path().join("hooks");
    std::fs::create_dir(&hooks_dir2).unwrap();
    let p2_json = serde_json::json!({
        HookEventType::SessionStart.as_str(): [
            { "type": "prompt", "prompt": "hi" }
        ]
    });
    std::fs::write(
        hooks_dir2.join("hooks.json"),
        serde_json::to_vec(&p2_json).unwrap(),
    )
    .unwrap();

    let p1 = test_plugin("plugin-a", dir1.path());
    let p2 = test_plugin("plugin-b", dir2.path());

    let hooks = load_all_plugin_hooks_v2(&[&p1, &p2]);
    assert_eq!(hooks.len(), 2);
    assert_eq!(hooks[0].event, HookEventType::PreToolUse);
    assert_eq!(hooks[1].event, HookEventType::SessionStart);
}

#[test]
fn test_status_message_preserves_existing() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join("hooks");
    std::fs::create_dir(&hooks_dir).unwrap();
    let hooks_json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [
            { "type": "command", "command": "echo x", "status_message": "linting code" }
        ]
    });
    std::fs::write(
        hooks_dir.join("hooks.json"),
        serde_json::to_vec(&hooks_json).unwrap(),
    )
    .unwrap();

    let plugin = test_plugin("lint-plugin", dir.path());
    let hooks = load_plugin_hooks_v2(&plugin);

    assert_eq!(hooks.len(), 1);
    assert_eq!(
        hooks[0].status_message.as_deref(),
        Some("[lint-plugin] linting code")
    );
}
