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

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;

const MAX_PLUGIN_RENAME_CHAIN: usize = 16;

static PLUGIN_ID_SCHEMA_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9][-A-Za-z0-9._]*@[A-Za-z0-9][-A-Za-z0-9._]*$")
        .unwrap_or_else(|e| unreachable!("static plugin-id regex failed to compile: {e}"))
});

/// Standard plugin directories.
pub fn get_plugin_dirs(config_dir: &Path, project_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // User-level plugins: config home/plugins/
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

    // Project-level plugins: project config dir/plugins/
    let project_plugins = project_dir
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("plugins");
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
fn read_enabled_disabled_ids(config_home: &Path) -> (HashSet<String>, HashSet<String>) {
    let mut enabled = HashSet::new();
    let mut disabled = HashSet::new();
    let path = config_home.join("settings.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
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
fn read_enabled_plugin_overrides(config_home: &Path) -> HashMap<String, bool> {
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
    let (mut enabled_ids, disabled_ids) = read_enabled_disabled_ids(config_home);

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
    let migrations = apply_marketplace_renames(&mut enabled_ids, &disabled_ids, &marketplaces);
    migrate_renamed_plugins_in_settings(config_home, project_dir, &migrations);

    let loader = loader::PluginLoader::new(plugins_dir);
    let standing = get_plugin_dirs(config_home, project_dir);
    loader
        .load_all_plugins(&standing, &marketplaces, &enabled_ids, &disabled_ids)
        .plugins
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PluginRenameResolution {
    Renamed { to: String, chain_depth: usize },
    Removed { chain_depth: usize },
    Unresolved { reason: RenameUnresolvedReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenameUnresolvedReason {
    Cycle,
    TargetMissing,
    ChainTooDeep,
}

impl RenameUnresolvedReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Cycle => "cycle",
            Self::TargetMissing => "target-missing",
            Self::ChainTooDeep => "chain-too-deep",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PluginRenameMigration {
    old_id: String,
    new_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PluginRenameTelemetryFields {
    outcome: &'static str,
    chain_depth: Option<usize>,
    reason: Option<&'static str>,
}

fn plugin_rename_telemetry_fields(
    resolution: &PluginRenameResolution,
) -> PluginRenameTelemetryFields {
    match resolution {
        PluginRenameResolution::Renamed { chain_depth, .. } => PluginRenameTelemetryFields {
            outcome: "renamed",
            chain_depth: Some(*chain_depth),
            reason: None,
        },
        PluginRenameResolution::Removed { chain_depth } => PluginRenameTelemetryFields {
            outcome: "removed",
            chain_depth: Some(*chain_depth),
            reason: None,
        },
        PluginRenameResolution::Unresolved { reason } => PluginRenameTelemetryFields {
            outcome: "unresolved",
            chain_depth: None,
            reason: Some(reason.as_str()),
        },
    }
}

fn emit_plugin_rename_telemetry(
    plugin_name: &str,
    marketplace: &str,
    resolution: &PluginRenameResolution,
) {
    let fields = plugin_rename_telemetry_fields(resolution);
    tracing::info!(
        target: "coco::plugins",
        event = "tengu_plugin_renamed",
        plugin = plugin_name,
        marketplace,
        outcome = fields.outcome,
        chain_depth = fields.chain_depth,
        reason = fields.reason,
        "tengu_plugin_renamed"
    );
}

fn resolve_plugin_rename(
    old_name: &str,
    renames: &HashMap<String, Option<String>>,
    present_plugin_names: &HashSet<String>,
) -> Option<PluginRenameResolution> {
    if !renames.contains_key(old_name) {
        return None;
    }
    let mut visited = HashSet::new();
    let mut current = old_name.to_string();
    for depth in 0..MAX_PLUGIN_RENAME_CHAIN {
        if !visited.insert(current.clone()) {
            return Some(PluginRenameResolution::Unresolved {
                reason: RenameUnresolvedReason::Cycle,
            });
        }
        match renames.get(&current) {
            None => {
                return Some(if present_plugin_names.contains(&current) {
                    PluginRenameResolution::Renamed {
                        to: current,
                        chain_depth: depth,
                    }
                } else {
                    PluginRenameResolution::Unresolved {
                        reason: RenameUnresolvedReason::TargetMissing,
                    }
                });
            }
            Some(None) => {
                return Some(PluginRenameResolution::Removed {
                    chain_depth: depth + 1,
                });
            }
            Some(Some(next)) => current = next.clone(),
        }
    }
    Some(PluginRenameResolution::Unresolved {
        reason: RenameUnresolvedReason::ChainTooDeep,
    })
}

fn apply_marketplace_renames(
    enabled_ids: &mut HashSet<String>,
    disabled_ids: &HashSet<String>,
    marketplaces: &[schemas::PluginMarketplace],
) -> Vec<PluginRenameMigration> {
    let marketplace_by_name: HashMap<&str, &schemas::PluginMarketplace> = marketplaces
        .iter()
        .map(|marketplace| (marketplace.name.as_str(), marketplace))
        .collect();
    let present_by_marketplace: HashMap<&str, HashSet<String>> = marketplaces
        .iter()
        .map(|marketplace| {
            (
                marketplace.name.as_str(),
                marketplace
                    .plugins
                    .iter()
                    .map(|entry| entry.name.clone())
                    .collect(),
            )
        })
        .collect();

    let mut migrations = Vec::new();
    for old_id in enabled_ids.clone() {
        let Some(id) = schemas::PluginId::parse(&old_id) else {
            continue;
        };
        let Some(marketplace) = marketplace_by_name.get(id.marketplace.as_str()) else {
            continue;
        };
        let Some(renames) = marketplace.renames.as_ref() else {
            continue;
        };
        let Some(present_names) = present_by_marketplace.get(id.marketplace.as_str()) else {
            continue;
        };
        if present_names.contains(&id.name) {
            continue;
        }
        match resolve_plugin_rename(&id.name, renames, present_names) {
            Some(PluginRenameResolution::Renamed { to, chain_depth }) => {
                let new_id = format!("{to}@{}", id.marketplace);
                if !PLUGIN_ID_SCHEMA_RE.is_match(&new_id) {
                    emit_plugin_rename_telemetry(
                        &id.name,
                        &id.marketplace,
                        &PluginRenameResolution::Unresolved {
                            reason: RenameUnresolvedReason::TargetMissing,
                        },
                    );
                    tracing::warn!(
                        old_id,
                        new_id,
                        "plugin rename target is not a valid plugin id; falling through to plugin-not-found"
                    );
                    continue;
                }
                enabled_ids.remove(&old_id);
                if !disabled_ids.contains(&new_id) {
                    enabled_ids.insert(new_id.clone());
                }
                migrations.push(PluginRenameMigration {
                    old_id,
                    new_id: Some(new_id.clone()),
                });
                emit_plugin_rename_telemetry(
                    &id.name,
                    &id.marketplace,
                    &PluginRenameResolution::Renamed { to, chain_depth },
                );
            }
            Some(PluginRenameResolution::Removed { chain_depth }) => {
                enabled_ids.remove(&old_id);
                migrations.push(PluginRenameMigration {
                    old_id,
                    new_id: None,
                });
                emit_plugin_rename_telemetry(
                    &id.name,
                    &id.marketplace,
                    &PluginRenameResolution::Removed { chain_depth },
                );
            }
            Some(PluginRenameResolution::Unresolved { reason }) => {
                emit_plugin_rename_telemetry(
                    &id.name,
                    &id.marketplace,
                    &PluginRenameResolution::Unresolved { reason },
                );
                tracing::warn!(
                    old_id,
                    reason = reason.as_str(),
                    "plugin has a renames entry but it does not resolve; falling through to plugin-not-found"
                );
            }
            None => {}
        }
    }
    migrations
}

fn migrate_renamed_plugins_in_settings(
    config_home: &Path,
    project_dir: &Path,
    migrations: &[PluginRenameMigration],
) {
    if migrations.is_empty() {
        return;
    }
    let paths = [
        config_home.join("settings.json"),
        project_dir
            .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("settings.json"),
        project_dir
            .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("settings.local.json"),
    ];
    let mut writes = 0;
    let mut failures = 0;
    for path in paths {
        match migrate_renamed_plugins_in_settings_file(&path, migrations) {
            SettingsMigrationOutcome::Written => writes += 1,
            SettingsMigrationOutcome::WriteFailed => failures += 1,
            SettingsMigrationOutcome::Unchanged => {}
        }
    }
    match (writes, failures) {
        (0, 0) => tracing::info!("plugin_rename_migration no_editable_scope"),
        (_, 0) => tracing::info!("plugin_rename_migration"),
        (_, _) if writes > 0 => tracing::warn!("plugin_rename_migration partial_settings_write"),
        _ => tracing::warn!("plugin_rename_migration settings_write_failed"),
    }
}

enum SettingsMigrationOutcome {
    Written,
    WriteFailed,
    Unchanged,
}

fn migrate_renamed_plugins_in_settings_file(
    path: &Path,
    migrations: &[PluginRenameMigration],
) -> SettingsMigrationOutcome {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return SettingsMigrationOutcome::Unchanged;
    };
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return SettingsMigrationOutcome::Unchanged;
    };
    let Some(root_obj) = root.as_object_mut() else {
        return SettingsMigrationOutcome::Unchanged;
    };
    let enabled_changed =
        migrate_plugin_id_map(root_obj, &["enabled_plugins", "enabledPlugins"], migrations);
    let configs_changed =
        migrate_plugin_id_map(root_obj, &["plugin_configs", "pluginConfigs"], migrations);
    let changed = enabled_changed || configs_changed;
    if !changed {
        return SettingsMigrationOutcome::Unchanged;
    }
    match serde_json::to_string_pretty(&root) {
        Ok(serialized) => {
            if let Err(e) = std::fs::write(path, serialized) {
                tracing::warn!(path = %path.display(), error = %e, "plugin_rename_migration settings_write_failed");
                SettingsMigrationOutcome::WriteFailed
            } else {
                SettingsMigrationOutcome::Written
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "plugin_rename_migration settings_serialize_failed");
            SettingsMigrationOutcome::WriteFailed
        }
    }
}

fn migrate_plugin_id_map(
    root_obj: &mut serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
    migrations: &[PluginRenameMigration],
) -> bool {
    let mut changed = false;
    for key in keys {
        let Some(map) = root_obj
            .get_mut(*key)
            .and_then(serde_json::Value::as_object_mut)
        else {
            continue;
        };

        for migration in migrations {
            let Some(value) = map.remove(&migration.old_id) else {
                continue;
            };
            changed = true;
            if let Some(new_id) = &migration.new_id
                && !map.contains_key(new_id)
            {
                map.insert(new_id.clone(), value);
            }
        }
    }
    changed
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
