//! Bridge between plugin contributions and the hook system.
//!
//! Converts plugin hook definitions (from manifest fields and `hooks/hooks.json`
//! files) into `HookDefinition` instances with plugin attribution.
//!

use coco_hooks::HookDefinition;
use coco_hooks::HookRegistry;
use coco_hooks::load_hooks_from_config;
use coco_types::HookScope;

use crate::loader::LoadedPluginV2;
use crate::schemas::ManifestHooks;
use crate::schemas::ManifestHooksEntry;

/// Load hook definitions from a plugin's hooks directory and manifest.
///
/// Hooks are loaded from:
/// 1. `hooks/hooks.json` in the plugin directory (if present)
/// 2. The manifest's `hooks` field — either inline JSON object or a file path
///    string pointing to a JSON file relative to the plugin root.
///
/// Each hook is tagged with `HookScope::Builtin` and has its `status_message`
/// prefixed with the plugin name for attribution.
pub fn load_plugin_hooks_v2(plugin: &LoadedPluginV2) -> Vec<HookDefinition> {
    let plugin_name = &plugin.id.name;
    let mut hooks = Vec::new();

    // 1. Load from hooks/hooks.json
    load_hooks_from_dir(&plugin.path, plugin_name, &mut hooks);

    // 2. Load from manifest hooks field
    if let Some(ref hooks_value) = plugin.manifest.hooks {
        load_hooks_from_manifest_value(hooks_value, &plugin.path, plugin_name, &mut hooks);
    }

    hooks
}

/// Load hooks from all enabled plugins.
pub fn load_all_plugin_hooks_v2(plugins: &[&LoadedPluginV2]) -> Vec<HookDefinition> {
    plugins
        .iter()
        .flat_map(|p| load_plugin_hooks_v2(p))
        .collect()
}

/// Register all plugin hooks into a `HookRegistry`, deduplicating.
///
/// Uses interior mutability — callers can pass `&Arc<HookRegistry>::deref()`
/// or a freshly-built `&HookRegistry`.
pub fn register_plugin_hooks_v2(registry: &HookRegistry, plugins: &[&LoadedPluginV2]) {
    for hook in load_all_plugin_hooks_v2(plugins) {
        registry.register_deduped(hook);
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Load hooks from a `hooks/hooks.json` file inside a plugin directory.
fn load_hooks_from_dir(
    plugin_path: &std::path::Path,
    plugin_name: &str,
    out: &mut Vec<HookDefinition>,
) {
    let hooks_json = plugin_path.join("hooks").join("hooks.json");
    load_hooks_from_file(&hooks_json, plugin_name, out);
}

/// Read a JSON file and parse its hooks.
fn load_hooks_from_file(path: &std::path::Path, plugin_name: &str, out: &mut Vec<HookDefinition>) {
    if !path.is_file() {
        return;
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                plugin = %plugin_name,
                path = %path.display(),
                "failed to read hooks file: {e}",
            );
            return;
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                plugin = %plugin_name,
                path = %path.display(),
                "failed to parse hooks JSON: {e}",
            );
            return;
        }
    };
    load_hooks_from_value(&value, plugin_name, out);
}

/// Parse a JSON value into hook definitions and tag them with plugin context.
fn load_hooks_from_value(
    value: &serde_json::Value,
    plugin_name: &str,
    out: &mut Vec<HookDefinition>,
) {
    match load_hooks_from_config(value, HookScope::Builtin) {
        Ok(defs) => {
            for mut hook in defs {
                tag_hook_with_plugin(&mut hook, plugin_name);
                out.push(hook);
            }
        }
        Err(e) => {
            tracing::warn!(
                plugin = %plugin_name,
                "failed to parse plugin hooks: {e}",
            );
        }
    }
}

/// Dispatch a typed `ManifestHooks` value into the appropriate loader.
fn load_hooks_from_manifest_value(
    hooks: &ManifestHooks,
    plugin_path: &std::path::Path,
    plugin_name: &str,
    out: &mut Vec<HookDefinition>,
) {
    match hooks {
        ManifestHooks::FilePath(path_str) => {
            let hooks_path = plugin_path.join(path_str);
            load_hooks_from_file(&hooks_path, plugin_name, out);
        }
        ManifestHooks::Inline(map) => {
            let value = serde_json::Value::Object(
                map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            );
            load_hooks_from_value(&value, plugin_name, out);
        }
        ManifestHooks::Multiple(entries) => {
            for entry in entries {
                match entry {
                    ManifestHooksEntry::FilePath(path_str) => {
                        let hooks_path = plugin_path.join(path_str);
                        load_hooks_from_file(&hooks_path, plugin_name, out);
                    }
                    ManifestHooksEntry::Inline(map) => {
                        let value = serde_json::Value::Object(
                            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                        );
                        load_hooks_from_value(&value, plugin_name, out);
                    }
                }
            }
        }
    }
}

/// Tag a hook definition with plugin attribution.
fn tag_hook_with_plugin(hook: &mut HookDefinition, plugin_name: &str) {
    hook.scope = HookScope::Builtin;
    hook.status_message = Some(match &hook.status_message {
        Some(msg) => format!("[{plugin_name}] {msg}"),
        None => format!("[{plugin_name}] running hook"),
    });
}

#[cfg(test)]
#[path = "hook_bridge.test.rs"]
mod tests;
