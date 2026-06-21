//! Plugin system: PLUGIN.toml / plugin.json manifests, marketplace cache, and
//! the V2 loader ([`loader::PluginLoader`] + [`load_enabled_plugins`]) that
//! resolves the active plugin set the session bootstrap registers contributions
//! from (commands / hooks / skills via the bridges).
//!

pub mod builtins;
pub mod command_bridge;
pub mod dependency;
pub mod errors;
pub mod fetch;
pub mod hints;
pub mod hook_bridge;
pub mod hot_reload;
pub mod identifier;
pub mod install;
pub mod loader;
pub mod lsp_bridge;
pub mod marketplace;
pub mod mcp_bridge;
pub mod mcpb;
pub mod official;
pub mod parse_marketplace_input;
pub mod schemas;
pub mod security;
pub mod skill_bridge;
pub mod versioning;
pub mod watcher;

pub use errors::PluginError;
pub use hints::ClaudeCodeHint;
pub use hints::extract_claude_code_hints;
pub use hints::pending_hint_snapshot;
pub use marketplace::MAX_SHOWN_PLUGINS;
pub use marketplace::PluginRecommendation;
pub use marketplace::detect_and_uninstall_delisted_plugins;
pub use marketplace::disable_hint_recommendations;
pub use marketplace::mark_hint_plugin_shown;
pub use marketplace::maybe_record_plugin_hint;
pub use marketplace::resolve_plugin_hint;
pub use marketplace::run_marketplace_startup;

/// Crate-local Result alias. Default error is `PluginError`; the open
/// generic preserves `Result::ok` and 2-arg `Result<T, E>` resolution.
pub type Result<T, E = PluginError> = std::result::Result<T, E>;

use std::path::Path;
use std::path::PathBuf;

/// Standard plugin directories.
pub fn get_plugin_dirs(config_dir: &Path, project_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // User-level plugins: ~/.coco/plugins/
    let user_plugins = config_dir.join("plugins");
    if user_plugins.is_dir()
        && let Ok(entries) = std::fs::read_dir(&user_plugins)
    {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                dirs.push(entry.path());
            }
        }
    }

    // Project-level plugins: .coco/plugins/
    let project_plugins = project_dir.join(".coco").join("plugins");
    if project_plugins.is_dir()
        && let Ok(entries) = std::fs::read_dir(&project_plugins)
    {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                dirs.push(entry.path());
            }
        }
    }

    dirs
}

/// Read settings.json `enabled_plugins` into `(enabled_ids, disabled_ids)` keyed
/// by the explicit boolean: `{ "enabled": true }` (or bare `true`) → enabled;
/// `false` → disabled; absent value defaults to enabled.
fn read_enabled_disabled_ids(
    config_home: &Path,
) -> (
    std::collections::HashSet<String>,
    std::collections::HashSet<String>,
) {
    let mut enabled = std::collections::HashSet::new();
    let mut disabled = std::collections::HashSet::new();
    let path = config_home.join("settings.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return (enabled, disabled);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return (enabled, disabled);
    };
    let Some(obj) = value
        .get("enabled_plugins")
        .or_else(|| value.get("enabledPlugins"))
        .and_then(|v| v.as_object())
    else {
        return (enabled, disabled);
    };
    for (id, v) in obj {
        let is_enabled = v
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .or_else(|| v.as_bool())
            .unwrap_or(true);
        if is_enabled {
            enabled.insert(id.clone());
        } else {
            disabled.insert(id.clone());
        }
    }
    (enabled, disabled)
}

/// Production entry point: load the full ENABLED plugin set from every source —
/// the marketplace versioned cache + local `inline` dirs — gated by settings.json
/// `enabled_plugins`. This is the single source the session bootstrap and
/// `/reload-plugins` register contributions from (commands / hooks / skills via
/// the V2 bridges).
pub fn load_enabled_plugins(config_home: &Path, project_dir: &Path) -> Vec<loader::LoadedPluginV2> {
    load_all_installed_plugins(config_home, project_dir)
        .into_iter()
        .filter(|p| p.enabled)
        .collect()
}

/// Read settings.json `enabled_plugins` into an id→bool override map (the shape
/// [`builtins::get_builtin_plugins`] consumes). Absent ids fall back to each
/// builtin's `default_enabled`.
fn read_enabled_plugin_overrides(config_home: &Path) -> std::collections::HashMap<String, bool> {
    let (enabled, disabled) = read_enabled_disabled_ids(config_home);
    enabled
        .into_iter()
        .map(|id| (id, true))
        .chain(disabled.into_iter().map(|id| (id, false)))
        .collect()
}

/// Skills contributed by enabled builtin plugins, honoring settings.json
/// enable/disable overrides. Empty until a builtin is registered via
/// [`builtins::init_builtin_plugins`].
pub fn builtin_plugin_skills(config_home: &Path) -> Vec<coco_skills::SkillDefinition> {
    builtins::get_builtin_plugin_skills(&read_enabled_plugin_overrides(config_home))
}

/// Like [`load_enabled_plugins`] but returns *every* installed plugin with its
/// resolved `enabled` flag (not just the enabled ones). Used by the
/// `/plugin enable|disable` handlers to resolve a bare name to its full
/// `name@marketplace` identity — including currently-disabled and
/// marketplace-installed plugins the standing-dir scan can't see.
pub fn load_all_installed_plugins(
    config_home: &Path,
    project_dir: &Path,
) -> Vec<loader::LoadedPluginV2> {
    let plugins_dir = config_home.join("plugins");
    let (enabled_ids, disabled_ids) = read_enabled_disabled_ids(config_home);

    // Marketplace catalogs (read every known marketplace's cached manifest).
    let mut mgr = marketplace::MarketplaceManager::new(plugins_dir.clone());
    let names: Vec<String> = mgr.load_known_marketplaces().into_keys().collect();
    for name in &names {
        let _ = mgr.load_cached_marketplace(name);
    }
    let marketplaces: Vec<schemas::PluginMarketplace> = names
        .iter()
        .filter_map(|n| mgr.cached_marketplace(n).cloned())
        .collect();

    let loader = loader::PluginLoader::new(plugins_dir);
    let standing = get_plugin_dirs(config_home, project_dir);
    loader
        .load_all_plugins(&standing, &marketplaces, &enabled_ids, &disabled_ids)
        .plugins
}

/// Discover each plugin's agent directories: the conventional
/// `<plugin>/agents/` dir plus any directory listed in the manifest `agents`
/// field. Returned as `(plugin_name, dir)` pairs so the subagent loader can
/// namespace each agent `<plugin>:<agent>` (single-file manifest entries are
/// not yet mapped).
pub fn plugin_agent_dirs(plugins: &[loader::LoadedPluginV2]) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for plugin in plugins {
        let agents_dir = plugin.path.join("agents");
        if agents_dir.is_dir() {
            out.push((plugin.id.name.clone(), agents_dir));
        }
        if let Some(paths) = &plugin.manifest.agents {
            for rel in paths.to_vec() {
                let dir = plugin.path.join(rel);
                if dir.is_dir() {
                    out.push((plugin.id.name.clone(), dir));
                }
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
