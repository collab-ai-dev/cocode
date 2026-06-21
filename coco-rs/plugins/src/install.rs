//! Shared plugin install pipeline for both the slash-command path
//! (`/plugin install`) and the CLI path (`coco plugin install`).
//!
//! Pipeline runs **five** steps:
//!
//! 1. **Policy guard** (root) — enterprise blocklist / allowlist.
//! 2. **Dependency closure** — DFS via [`crate::dependency::resolve_dependency_closure`].
//! 3. **Policy guard** (every dep) — closure-wide.
//! 4. **Settings write** — entire closure persisted into
//!    `enabled_plugins` on the target settings.json so the next session
//!    auto-loads the plugin set.
//! 5. **Materialize** — cache & register each closure member.
//!
//! The pipeline is intentionally pure of UI / println so the slash
//! handler can return strings while the CLI handler can `println!`.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use chrono::Utc;
use thiserror::Error;

use crate::dependency::DependencyLookupResult;
use crate::dependency::ResolutionResult;
use crate::dependency::resolve_dependency_closure;
use crate::identifier::PluginId;
use crate::loader::InstalledPluginsManager;
use crate::marketplace::MarketplaceManager;
use crate::schemas::PluginInstallationEntry;
use crate::schemas::PluginMarketplaceEntry;
use crate::schemas::PluginScope;
use crate::security::EnterprisePolicy;
use crate::security::PolicyVerdict;
use crate::security::check_policy;

/// Successful resolution + materialisation of an install.
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    /// Fully-qualified plugin ID (`name@marketplace`).
    pub plugin_id: String,
    /// On-disk location of the installed root plugin.
    pub install_path: PathBuf,
    /// Marketplace the plugin came from.
    pub marketplace_name: String,
    /// Plugin name (root only).
    pub plugin_name: String,
    /// Closure resolved during install (always includes the root).
    pub closure: Vec<PluginId>,
    /// Suffix string for install-success messages (`" (+ 2 dependencies)"`).
    pub dep_note: String,
}

/// Why an install attempt did not produce an [`InstallOutcome`].
#[derive(Debug, Error)]
pub enum InstallError {
    /// No marketplaces have ever been registered. The user must run
    /// `/plugin marketplace add <source>` first.
    #[error("no marketplaces configured")]
    NoMarketplacesConfigured,

    /// Plugin name (with or without `@marketplace`) did not resolve to
    /// any cached marketplace entry.
    #[error("plugin '{plugin_name}' not found{}",
        marketplace_filter.as_ref()
            .map(|m| format!(" in marketplace '{m}'"))
            .unwrap_or_default())]
    NotFound {
        plugin_name: String,
        marketplace_filter: Option<String>,
    },

    /// Root plugin blocked by enterprise policy.
    #[error("plugin '{plugin_name}' is blocked by enterprise policy ({reason})")]
    BlockedByPolicy { plugin_name: String, reason: String },

    /// A transitive dependency is blocked by policy.
    #[error(
        "cannot install '{plugin_name}': dependency '{dependency}' is blocked by enterprise policy ({reason})"
    )]
    DependencyBlockedByPolicy {
        plugin_name: String,
        dependency: String,
        reason: String,
    },

    /// Dependency resolution failed (cycle / cross-marketplace / not
    /// found). The string is shaped for direct user display.
    #[error("{0}")]
    ResolutionFailed(String),

    /// Settings.json write failed (I/O / serialization).
    #[error("failed to update settings: {0}")]
    SettingsWriteFailed(String),

    /// Underlying plugin-system error (I/O, schema, marketplace fetch).
    #[error(transparent)]
    Other(#[from] crate::PluginError),
}

/// Parse `<name>[@<marketplace>]` into the pair the resolver expects.
///
/// Trim is applied so users can paste copy-with-whitespace identifiers
/// without surprises.
pub(crate) fn parse_install_target(target: &str) -> (String, Option<String>) {
    let trimmed = target.trim();
    match trimmed.split_once('@') {
        Some((name, mkt)) => (name.trim().to_string(), Some(mkt.trim().to_string())),
        None => (trimmed.to_string(), None),
    }
}

/// Drive the shared install pipeline.
///
/// Steps:
/// 1. Resolve `target` to `(marketplace_name, entry)`.
/// 2. Check root against `policy`.
/// 3. Resolve transitive dependency closure.
/// 4. Check each closure member against `policy`.
/// 5. Persist `enabled_plugins` into the user settings.json (`settings_dir`).
/// 6. Cache + register each closure member.
///
/// `settings_dir` is the directory containing `settings.json` to update
/// (typically `~/.coco`). When `None`, the settings write is skipped.
pub async fn install_plugin_from_marketplace(
    plugins_dir: &Path,
    settings_dir: Option<&Path>,
    policy: &EnterprisePolicy,
    target: &str,
    scope: PluginScope,
) -> Result<InstallOutcome, InstallError> {
    let (plugin_name, marketplace_filter) = parse_install_target(target);

    let mut manager = MarketplaceManager::new(plugins_dir.to_path_buf());

    let known = manager.load_known_marketplaces();
    if known.is_empty() {
        return Err(InstallError::NoMarketplacesConfigured);
    }
    for name in known.keys() {
        let _ = manager.load_cached_marketplace(name);
    }

    let resolved = if let Some(mkt) = marketplace_filter.as_deref() {
        manager
            .get_plugin_by_id(&format!("{plugin_name}@{mkt}"))
            .map(|(_, entry)| (mkt.to_string(), entry.clone()))
    } else {
        manager
            .search_plugins(&plugin_name)
            .into_iter()
            .find(|p| p.name == plugin_name)
            .and_then(|p| {
                manager
                    .get_plugin_by_id(&format!("{}@{}", p.name, p.marketplace))
                    .map(|(_, e)| (p.marketplace, e.clone()))
            })
    };

    let Some((marketplace_name, entry)) = resolved else {
        return Err(InstallError::NotFound {
            plugin_name,
            marketplace_filter,
        });
    };

    let is_user_scope = matches!(scope, PluginScope::User);
    let root_id = PluginId::new(entry.name.clone(), marketplace_name.clone());

    // Step 2: policy guard (root).
    match check_policy(&root_id, is_user_scope, policy) {
        PolicyVerdict::Ok => {}
        verdict => {
            return Err(InstallError::BlockedByPolicy {
                plugin_name: entry.name.clone(),
                reason: policy_reason(&verdict),
            });
        }
    }

    // Step 3: dependency closure.
    //
    // We snapshot every marketplace's entries (name → deps) into a
    // local map and serve the resolver from that — avoids holding the
    // mutable marketplace manager borrow across `.await`.
    let lookup_map = collect_dependency_lookup(&manager);
    let already_enabled = read_enabled_plugins(settings_dir);
    let allowed_cross = root_marketplace_allowed_cross(&manager, &marketplace_name);
    let resolution = resolve_dependency_closure(
        &root_id,
        |id| {
            let lookup_map = lookup_map.clone();
            async move { lookup_map.get(&id).cloned() }
        },
        &already_enabled,
        &allowed_cross,
    )
    .await;
    let closure = match resolution {
        ResolutionResult::Ok { closure } => closure,
        other => return Err(InstallError::ResolutionFailed(format_resolution(&other))),
    };

    // Step 4: policy guard (every dep, root already checked).
    for dep_id in &closure {
        if dep_id == &root_id {
            continue;
        }
        match check_policy(dep_id, is_user_scope, policy) {
            PolicyVerdict::Ok => {}
            verdict => {
                return Err(InstallError::DependencyBlockedByPolicy {
                    plugin_name: entry.name.clone(),
                    dependency: dep_id.to_string(),
                    reason: policy_reason(&verdict),
                });
            }
        }
    }

    // Step 5: materialize the root plugin FIRST. Nothing is persisted to
    // settings.json / installed-state until the content actually exists on
    // disk, so a failed install (e.g. an unsupported remote source) leaves no
    // broken enabled-but-empty entry behind — the model/user sees a real
    // failure instead of a phantom "✓ Installed".
    let install_path = manager
        .install_plugin(&marketplace_name, &entry, scope)
        .await
        .map_err(InstallError::from)?;

    // Step 6: persist enabledPlugins in settings.json (best-effort —
    // every closure member becomes `{ "enabled": true }`).
    if let Some(dir) = settings_dir
        && let Err(e) = write_enabled_plugins(dir, &closure)
    {
        return Err(InstallError::SettingsWriteFailed(e.to_string()));
    }

    record_installation(
        plugins_dir,
        &root_id.to_string(),
        &install_path,
        &entry,
        scope,
    )?;

    for dep_id in &closure {
        if dep_id == &root_id {
            continue;
        }
        let Some(dep_mkt) = &dep_id.marketplace else {
            continue;
        };
        if let Some((_, dep_entry)) = manager.get_plugin_by_id(&dep_id.to_string()) {
            let dep_entry = dep_entry.clone();
            match manager.install_plugin(dep_mkt, &dep_entry, scope).await {
                Ok(dep_path) => {
                    if let Err(e) = record_installation(
                        plugins_dir,
                        &dep_id.to_string(),
                        &dep_path,
                        &dep_entry,
                        scope,
                    ) {
                        tracing::warn!(plugin = %dep_id, "failed to record dep install: {e}");
                    }
                }
                Err(e) => tracing::warn!(plugin = %dep_id, "failed to install dep: {e}"),
            }
        }
    }

    // depNote = formatDependencyCountSuffix(closure excluding root).
    let installed_deps: Vec<PluginId> = closure
        .iter()
        .filter(|id| *id != &root_id)
        .cloned()
        .collect();
    let dep_note = crate::dependency::format_dependency_count_suffix(&installed_deps);
    Ok(InstallOutcome {
        plugin_id: root_id.to_string(),
        install_path,
        marketplace_name,
        plugin_name: entry.name.clone(),
        closure,
        dep_note,
    })
}

// ─── helpers ────────────────────────────────────────────────────────────

/// Snapshot every cached marketplace's plugins into a `PluginId →
/// dependencies` map for the dep resolver.
fn collect_dependency_lookup(
    manager: &MarketplaceManager,
) -> HashMap<PluginId, DependencyLookupResult> {
    let mut out = HashMap::new();
    for known_name in manager.load_known_marketplaces().keys() {
        if let Some(marketplace) = manager.cached_marketplace(known_name) {
            for entry in &marketplace.plugins {
                let id = PluginId::new(entry.name.clone(), known_name.clone());
                out.insert(
                    id,
                    DependencyLookupResult {
                        dependencies: entry.dependencies.clone().unwrap_or_default(),
                    },
                );
            }
        }
    }
    out
}

/// Look up `allow_cross_marketplace_dependencies_on` on the named
/// marketplace. Empty set when the field is unset.
fn root_marketplace_allowed_cross(
    manager: &MarketplaceManager,
    marketplace_name: &str,
) -> HashSet<String> {
    manager
        .cached_marketplace(marketplace_name)
        .and_then(|m| m.allow_cross_marketplace_dependencies_on.clone())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Read the current `enabled_plugins` keys from `<settings_dir>/settings.json`.
/// Returns an empty set when the file is missing / malformed.
fn read_enabled_plugins(settings_dir: Option<&Path>) -> HashSet<PluginId> {
    let Some(dir) = settings_dir else {
        return HashSet::new();
    };
    let path = dir.join("settings.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return HashSet::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return HashSet::new();
    };
    let Some(obj) = value
        .get("enabled_plugins")
        .or_else(|| value.get("enabledPlugins"))
        .and_then(|v| v.as_object())
    else {
        return HashSet::new();
    };
    obj.keys().map(|k| PluginId::parse(k)).collect()
}

/// Write the closure as `enabled_plugins: { "<id>": { "enabled": true } }`
/// to `<settings_dir>/settings.json`, preserving every other field
/// already in the file.
fn write_enabled_plugins(settings_dir: &Path, closure: &[PluginId]) -> std::io::Result<()> {
    let path = settings_dir.join("settings.json");
    std::fs::create_dir_all(settings_dir)?;
    let mut value: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(e),
    };

    let map = value
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("settings.json is not a JSON object"))?;

    let entries = map
        .entry("enabled_plugins".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let entries_map = entries
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("enabled_plugins is not a JSON object"))?;
    for id in closure {
        entries_map.insert(id.to_string(), serde_json::json!({ "enabled": true }));
    }

    let json =
        serde_json::to_string_pretty(&value).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Read the explicit `enabled_plugins[<id>].enabled` flag from settings.json.
/// `None` when there is no entry (the plugin is enabled by default). Accepts
/// both `{ "enabled": bool }` and a bare-bool legacy shape.
pub fn read_plugin_enabled(settings_dir: &Path, plugin_id: &PluginId) -> Option<bool> {
    let path = settings_dir.join("settings.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let obj = value
        .get("enabled_plugins")
        .or_else(|| value.get("enabledPlugins"))
        .and_then(|v| v.as_object())?;
    let entry = obj.get(&plugin_id.to_string())?;
    entry
        .get("enabled")
        .and_then(serde_json::Value::as_bool)
        .or_else(|| entry.as_bool())
}

/// Set `enabled_plugins[<id>] = { "enabled": <enabled> }` in settings.json,
/// preserving every other field. This is the single source of truth the loader
/// and policy layer read, so `/plugin enable|disable` and the install path
/// write the same place.
pub fn set_plugin_enabled(
    settings_dir: &Path,
    plugin_id: &PluginId,
    enabled: bool,
) -> std::io::Result<()> {
    let path = settings_dir.join("settings.json");
    std::fs::create_dir_all(settings_dir)?;
    let mut value: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(e),
    };
    let map = value
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("settings.json is not a JSON object"))?;
    let entries = map
        .entry("enabled_plugins".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let entries_map = entries
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("enabled_plugins is not a JSON object"))?;
    entries_map.insert(
        plugin_id.to_string(),
        serde_json::json!({ "enabled": enabled }),
    );
    let json =
        serde_json::to_string_pretty(&value).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(&path, json)?;
    Ok(())
}

fn policy_reason(verdict: &PolicyVerdict) -> String {
    match verdict {
        PolicyVerdict::BlockedPlugin { plugin } => {
            format!("plugin '{plugin}' is blocked by your organization's policy")
        }
        PolicyVerdict::BlockedMarketplace { marketplace } => {
            format!("marketplace '{marketplace}' is blocklisted")
        }
        PolicyVerdict::UnapprovedMarketplace { marketplace } => {
            format!("marketplace '{marketplace}' is not in the approved allowlist")
        }
        PolicyVerdict::UserScopeForbidden => "user-scope installs are disabled".to_string(),
        PolicyVerdict::Ok => String::new(),
    }
}

/// Format a resolution error for display.
fn format_resolution(r: &ResolutionResult) -> String {
    match r {
        ResolutionResult::Cycle { chain } => format!(
            "Dependency cycle: {}",
            chain
                .iter()
                .map(PluginId::to_string)
                .collect::<Vec<_>>()
                .join(" → ")
        ),
        ResolutionResult::CrossMarketplace {
            dependency,
            required_by,
        } => format!(
            "Dependency '{dependency}' (required by {required_by}) is in a different marketplace \
             — cross-marketplace dependencies are blocked by default. Install it manually first, \
             or add it to the root marketplace's allowed-cross-marketplace allowlist."
        ),
        ResolutionResult::NotFound {
            missing,
            required_by,
        } => format!(
            "Dependency '{missing}' (required by {required_by}) not found in any configured \
             marketplace. Is the '{}' marketplace added?",
            missing.marketplace.as_deref().unwrap_or("unknown")
        ),
        ResolutionResult::Ok { .. } => String::new(),
    }
}

/// Append (or replace) the install record so the next session loader
/// picks the plugin up automatically.
fn record_installation(
    plugins_dir: &Path,
    plugin_id: &str,
    install_path: &Path,
    entry: &PluginMarketplaceEntry,
    scope: PluginScope,
) -> Result<(), InstallError> {
    let installed_path = plugins_dir.join("installed_plugins.json");
    let mut installed =
        InstalledPluginsManager::load(installed_path).map_err(InstallError::from)?;
    let now = Utc::now().to_rfc3339();
    installed.record_installation(
        plugin_id,
        PluginInstallationEntry {
            scope,
            project_path: None,
            install_path: install_path.to_string_lossy().to_string(),
            version: entry.version.clone(),
            installed_at: Some(now.clone()),
            last_updated: Some(now),
            git_commit_sha: None,
        },
    );
    installed.save().map_err(InstallError::from)?;
    Ok(())
}

#[cfg(test)]
#[path = "install.test.rs"]
mod tests;
