//! Bridge between plugin contributions and the LSP subsystem.
//!
//! Converts a plugin's declared LSP servers into a [`LspServersConfig`] the
//! session merges into the live `LspServerManager` (via its `merge_config`
//! seam). Server names are namespaced `plugin:<plugin>:<server>` and each
//! server's environment gets `COCO_PLUGIN_ROOT` injected.
//!

use std::collections::HashMap;
use std::path::Path;

use coco_lsp::LspServerConfig;
use coco_lsp::LspServersConfig;
use serde_json::Value;

use crate::loader::LoadedPluginV2;

/// The env var a plugin's LSP server process can reference to locate its
/// install root. `${CLAUDE_PLUGIN_ROOT}` / `${COCO_PLUGIN_ROOT}` tokens in
/// `command`/`args` are also substituted.
const PLUGIN_ROOT_ENV: &str = "COCO_PLUGIN_ROOT";

/// Collect the namespaced LSP servers contributed by every plugin in `plugins`.
///
/// Each enabled plugin's servers (from `<root>/.lsp.json` + manifest
/// `lsp_servers`) are keyed `plugin:<plugin>:<server>` and merged into one
/// config. Later plugins win on key collision (the namespace makes collisions
/// rare).
pub fn extract_lsp_servers_from_plugins(plugins: &[&LoadedPluginV2]) -> LspServersConfig {
    let mut merged = LspServersConfig::default();
    for plugin in plugins {
        for (name, config) in load_plugin_lsp_servers(plugin) {
            merged.servers.insert(name, config);
        }
    }
    merged
}

/// Load one plugin's LSP servers, namespaced and env-resolved.
///
/// Sources, lowest-priority first (later overrides earlier by server name):
/// 1. `<plugin root>/.lsp.json` — a `{ server: config }` record.
/// 2. Manifest `lsp_servers` — a path string, an inline `{ server: config }`
///    object, or an array mixing the two.
pub fn load_plugin_lsp_servers(plugin: &LoadedPluginV2) -> HashMap<String, LspServerConfig> {
    let plugin_name = &plugin.id.name;
    let mut servers: HashMap<String, LspServerConfig> = HashMap::new();

    // 1. <root>/.lsp.json
    let dot_lsp = plugin.path.join(".lsp.json");
    if dot_lsp.is_file() {
        merge_record_file(&dot_lsp, &mut servers);
    }

    // 2. manifest `lsp_servers`
    if let Some(value) = &plugin.manifest.lsp_servers {
        merge_manifest_value(value, &plugin.path, &mut servers);
    }

    // Namespace + env-resolve every server.
    servers
        .into_iter()
        .map(|(name, mut config)| {
            resolve_env(&mut config, &plugin.path);
            (format!("plugin:{plugin_name}:{name}"), config)
        })
        .collect()
}

/// Merge a manifest `lsp_servers` value (string path / inline object / array).
fn merge_manifest_value(
    value: &Value,
    plugin_root: &Path,
    out: &mut HashMap<String, LspServerConfig>,
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

/// Read a `{ server: config }` record JSON file and merge it.
fn merge_record_file(path: &Path, out: &mut HashMap<String, LspServerConfig>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(value) => merge_record_value(&value, out),
        Err(e) => tracing::warn!(path = %path.display(), "invalid plugin LSP config: {e}"),
    }
}

/// Merge a `{ server: entry }` JSON object into `out`, accepting both the
/// native coco-lsp field shape and the `extensionToLanguage` map.
fn merge_record_value(value: &Value, out: &mut HashMap<String, LspServerConfig>) {
    let Some(map) = value.as_object() else {
        return;
    };
    for (name, entry) in map {
        if let Some(config) = parse_server_entry(entry) {
            out.insert(name.clone(), config);
        }
    }
}

/// Convert one server entry into an [`LspServerConfig`]. Native fields
/// (`file_extensions` / `languages`) deserialize directly; an
/// `extensionToLanguage` map (if present) is folded into both vecs.
fn parse_server_entry(entry: &Value) -> Option<LspServerConfig> {
    let mut config: LspServerConfig = serde_json::from_value(entry.clone())
        .map_err(|e| tracing::warn!("invalid plugin LSP server entry: {e}"))
        .ok()?;

    if let Some(map) = entry.get("extensionToLanguage").and_then(Value::as_object) {
        for (ext, lang) in map {
            if !config.file_extensions.contains(ext) {
                config.file_extensions.push(ext.clone());
            }
            if let Some(lang) = lang.as_str()
                && !config.languages.iter().any(|l| l == lang)
            {
                config.languages.push(lang.to_string());
            }
        }
    }
    Some(config)
}

/// Inject `COCO_PLUGIN_ROOT` and substitute the plugin-root token in
/// `command` / `args` (`${user_config.X}` substitution is not supported).
fn resolve_env(config: &mut LspServerConfig, plugin_root: &Path) {
    let root = plugin_root.display().to_string();
    config
        .env
        .entry(PLUGIN_ROOT_ENV.to_string())
        .or_insert_with(|| root.clone());
    if let Some(command) = &mut config.command {
        *command = substitute_root(command, &root);
    }
    for arg in &mut config.args {
        *arg = substitute_root(arg, &root);
    }
}

fn substitute_root(s: &str, root: &str) -> String {
    s.replace("${CLAUDE_PLUGIN_ROOT}", root)
        .replace("${COCO_PLUGIN_ROOT}", root)
}

/// Resolve `rel` against `plugin_root`, rejecting paths that escape the plugin
/// directory. Returns `None` if the resolved path is missing or outside the root.
fn resolve_within(plugin_root: &Path, rel: &str) -> Option<std::path::PathBuf> {
    let candidate = plugin_root.join(rel);
    let canonical = candidate.canonicalize().ok()?;
    let root = plugin_root.canonicalize().ok()?;
    canonical.starts_with(&root).then_some(canonical)
}

#[cfg(test)]
#[path = "lsp_bridge.test.rs"]
mod tests;
