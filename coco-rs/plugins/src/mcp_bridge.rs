//! Bridge between plugin contributions and the MCP subsystem.
//!
//! Converts a plugin's declared MCP servers into namespaced
//! [`ScopedMcpServerConfig`]s the session registers with the
//! `McpConnectionManager`. Server names are namespaced
//! `plugin:<plugin>:<server>`, scoped [`ConfigScope::Dynamic`], and tagged
//! with `plugin_source`. Stdio servers get the plugin root injected into env.
//!

use std::path::Path;

use coco_mcp::ConfigScope;
use coco_mcp::McpServerConfig;
use coco_mcp::ScopedMcpServerConfig;
use coco_mcp::config::parse_server_config;
use serde_json::Value;

use crate::loader::LoadedPluginV2;

/// Env vars a plugin's MCP server process can read to locate its install root.
/// `COCO_PLUGIN_ROOT` is the coco-native name; `CLAUDE_PLUGIN_ROOT` is injected
/// too for compatibility with servers ported from Claude Code plugins.
const PLUGIN_ROOT_ENV: [&str; 2] = ["COCO_PLUGIN_ROOT", "CLAUDE_PLUGIN_ROOT"];

/// Collect the namespaced MCP servers contributed by every plugin in `plugins`.
///
/// Each enabled plugin's servers (manifest `mcp_servers` + a `<root>/.mcp.json`)
/// are keyed `plugin:<plugin>:<server>`, scoped `Dynamic`, and tagged with the
/// plugin id.
pub fn extract_mcp_servers_from_plugins(plugins: &[&LoadedPluginV2]) -> Vec<ScopedMcpServerConfig> {
    plugins
        .iter()
        .flat_map(|plugin| load_plugin_mcp_servers(plugin))
        .collect()
}

/// Load one plugin's MCP servers, namespaced and env-resolved.
///
/// Sources, lowest-priority first (later overrides earlier by server name):
/// 1. `<plugin root>/.mcp.json` — `{ mcpServers: {...} }` or a bare `{...}` map.
/// 2. Manifest `mcp_servers` — a path string, an inline `{ server: config }`
///    object, or an array mixing the two.
pub fn load_plugin_mcp_servers(plugin: &LoadedPluginV2) -> Vec<ScopedMcpServerConfig> {
    let plugin_name = &plugin.id.name;
    let plugin_id = plugin.id.to_string();
    let mut servers: Vec<(String, McpServerConfig)> = Vec::new();

    // 1. <root>/.mcp.json
    let dot_mcp = plugin.path.join(".mcp.json");
    if dot_mcp.is_file() {
        merge_record_file(&dot_mcp, &mut servers);
    }

    // 2. manifest `mcp_servers`
    if let Some(value) = &plugin.manifest.mcp_servers {
        merge_manifest_value(value, &plugin.path, &mut servers);
    }

    servers
        .into_iter()
        .map(|(name, mut config)| {
            inject_plugin_root(&mut config, &plugin.path);
            ScopedMcpServerConfig {
                name: format!("plugin:{plugin_name}:{name}"),
                config,
                scope: ConfigScope::Dynamic,
                plugin_source: Some(plugin_id.clone()),
            }
        })
        .collect()
}

/// Merge a manifest `mcp_servers` value (string path / inline object / array).
fn merge_manifest_value(
    value: &Value,
    plugin_root: &Path,
    out: &mut Vec<(String, McpServerConfig)>,
) {
    match value {
        Value::String(rel) => {
            if let Some(path) = resolve_within(plugin_root, rel) {
                merge_record_file(&path, out);
            }
        }
        Value::Object(_) => merge_record_value(value, out),
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(rel) => {
                        if let Some(path) = resolve_within(plugin_root, rel) {
                            merge_record_file(&path, out);
                        }
                    }
                    Value::Object(_) => merge_record_value(item, out),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Read a server-record JSON file (accepting `{mcpServers: {...}}` or a bare
/// `{...}` map) and merge it.
fn merge_record_file(path: &Path, out: &mut Vec<(String, McpServerConfig)>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(value) => {
            let record = value.get("mcpServers").unwrap_or(&value);
            merge_record_value(record, out);
        }
        Err(e) => tracing::warn!(path = %path.display(), "invalid plugin MCP config: {e}"),
    }
}

/// Merge a `{ server: entry }` JSON object, parsing each entry into an
/// [`McpServerConfig`]. A later entry with the same name overrides an earlier.
fn merge_record_value(value: &Value, out: &mut Vec<(String, McpServerConfig)>) {
    let Some(map) = value.as_object() else {
        return;
    };
    for (name, entry) in map {
        if let Some(config) = parse_server_config(entry) {
            if let Some(slot) = out.iter_mut().find(|(n, _)| n == name) {
                slot.1 = config;
            } else {
                out.push((name.clone(), config));
            }
        }
    }
}

/// Inject the plugin-root env vars into a stdio server's environment and
/// substitute the plugin-root token in command/args.
fn inject_plugin_root(config: &mut McpServerConfig, plugin_root: &Path) {
    let McpServerConfig::Stdio(stdio) = config else {
        return;
    };
    let root = plugin_root.display().to_string();
    for key in PLUGIN_ROOT_ENV {
        stdio
            .env
            .entry(key.to_string())
            .or_insert_with(|| root.clone());
    }
    stdio.command = substitute_root(&stdio.command, &root);
    for arg in &mut stdio.args {
        *arg = substitute_root(arg, &root);
    }
}

fn substitute_root(s: &str, root: &str) -> String {
    s.replace("${CLAUDE_PLUGIN_ROOT}", root)
        .replace("${COCO_PLUGIN_ROOT}", root)
}

/// Resolve `rel` against `plugin_root`, rejecting paths that escape the plugin
/// directory. Returns `None` if the resolved path is missing or outside.
fn resolve_within(plugin_root: &Path, rel: &str) -> Option<std::path::PathBuf> {
    let canonical = plugin_root.join(rel).canonicalize().ok()?;
    let root = plugin_root.canonicalize().ok()?;
    canonical.starts_with(&root).then_some(canonical)
}

#[cfg(test)]
#[path = "mcp_bridge.test.rs"]
mod tests;
