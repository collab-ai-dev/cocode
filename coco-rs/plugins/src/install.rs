//! Shared plugin install pipeline for both the slash-command path
//! (`/plugin install`) and the CLI path (`coco plugin install`).
//!
//! Pipeline runs as one fail-closed dependency-closure transaction:
//!
//! 1. Resolve and policy-check the complete closure.
//! 2. Materialize and structurally inspect every artifact under staging.
//! 3. Publish every artifact with rollback backups.
//! 4. Atomically replace the installation ledger.
//! 5. Atomically enable the closure in settings as the activation point.
//!
//! The pipeline is intentionally pure of UI / println so the slash
//! handler can return strings while the CLI handler can `println!`.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;

use chrono::Utc;
use fs2::FileExt;
use thiserror::Error;

use crate::dependency::DependencyLookupResult;
use crate::dependency::ResolutionResult;
use crate::dependency::resolve_dependency_closure;
use crate::identifier::PluginId;
use crate::loader::InstalledPluginsManager;
use crate::loader::PluginLoadSource;
use crate::loader::PluginLoader;
use crate::marketplace::MarketplaceManager;
use crate::schemas::PluginInstallationEntry;
use crate::schemas::PluginMarketplaceEntry;
use crate::schemas::PluginScope;
use crate::security::EnterprisePolicy;
use crate::security::PolicyVerdict;
use crate::security::check_policy;

static INSTALL_MUTEX: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

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

    #[error("staged plugin '{plugin}' is invalid: {reason}")]
    InvalidArtifact { plugin: String, reason: String },

    #[error("plugin install rollback failed after {operation}: {reason}")]
    RollbackFailed { operation: String, reason: String },

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
/// The complete dependency closure is staged and validated before any live
/// path changes. Publishing, ledger persistence, and settings activation are
/// rollback-protected by a process mutex and a cross-process file lock.
///
/// `settings_dir` is the directory containing `settings.json` to update
/// (typically the config home). When `None`, the settings write is skipped.
pub async fn install_plugin_from_marketplace(
    plugins_dir: &Path,
    settings_dir: Option<&Path>,
    policy: &EnterprisePolicy,
    target: &str,
    scope: PluginScope,
) -> Result<InstallOutcome, InstallError> {
    let _process_guard = INSTALL_MUTEX.lock().await;
    let _file_guard = acquire_install_lock(plugins_dir).await?;
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
    let allowed_cross = root_marketplace_allowed_cross(&manager, &marketplace_name);
    let resolution = resolve_dependency_closure(
        &root_id,
        |id| {
            let lookup_map = lookup_map.clone();
            async move { lookup_map.get(&id).cloned() }
        },
        // Re-materialize the whole closure. Settings alone are not proof that
        // an installed dependency still has a valid artifact on disk.
        &HashSet::new(),
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

    let staging_parent = plugins_dir.join(".staging");
    let staging = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&staging_parent)?;
        tempfile::Builder::new()
            .prefix("install-")
            .tempdir_in(&staging_parent)
            .map_err(crate::PluginError::from)
    })
    .await
    .map_err(|error| {
        InstallError::Other(crate::PluginError::generic(
            "install",
            format!("plugin staging task failed: {error}"),
        ))
    })??;
    let mut staged = Vec::with_capacity(closure.len());

    for (index, id) in closure.iter().enumerate() {
        let dependency_marketplace = id.marketplace.as_deref().ok_or_else(|| {
            InstallError::ResolutionFailed(format!(
                "resolved plugin '{id}' has no marketplace identity"
            ))
        })?;
        let (_, dependency_entry) = manager.get_plugin_by_id(&id.to_string()).ok_or_else(|| {
            InstallError::ResolutionFailed(format!(
                "resolved plugin '{id}' disappeared from its marketplace"
            ))
        })?;
        let dependency_entry = dependency_entry.clone();
        let install_path = manager.install_path(dependency_marketplace, &dependency_entry);
        let stage_path = staging.path().join(index.to_string());
        manager
            .materialize_plugin_to(dependency_marketplace, &dependency_entry, &stage_path)
            .await?;
        let inspection_path = stage_path.clone();
        let loader_root = plugins_dir.to_path_buf();
        let marketplace = dependency_marketplace.to_string();
        let plugin_label = id.to_string();
        let (inspection, loaded) = tokio::task::spawn_blocking(move || {
            let inspection = crate::artifact::inspect_artifact(&inspection_path)?;
            let loader = PluginLoader::new(loader_root);
            let loaded = loader
                .load_from_dir(
                    &inspection_path,
                    PluginLoadSource::Marketplace {
                        marketplace: marketplace.clone(),
                    },
                    Some(&marketplace),
                )
                .map_err(|error| InstallError::InvalidArtifact {
                    plugin: plugin_label,
                    reason: error.to_string(),
                })?;
            Ok::<_, InstallError>((inspection, loaded))
        })
        .await
        .map_err(|error| {
            InstallError::Other(crate::PluginError::generic(
                "install",
                format!("plugin inspection task failed: {error}"),
            ))
        })??;
        if loaded.id.name != id.name {
            return Err(InstallError::InvalidArtifact {
                plugin: id.to_string(),
                reason: format!(
                    "manifest name '{}' does not match marketplace entry '{}'",
                    loaded.id.name, id.name
                ),
            });
        }
        staged.push(StagedPlugin {
            id: id.clone(),
            entry: dependency_entry,
            stage_path,
            install_path,
            inspection,
        });
    }

    let install_path = staged
        .iter()
        .find(|plugin| plugin.id == root_id)
        .map(|plugin| plugin.install_path.clone())
        .ok_or_else(|| {
            InstallError::ResolutionFailed("root missing from install closure".into())
        })?;
    let rollback_parent = staging.path().join("rollback");
    let commit_plugins_dir = plugins_dir.to_path_buf();
    let commit_settings_dir = settings_dir.map(Path::to_path_buf);
    let commit_closure = closure.clone();
    tokio::task::spawn_blocking(move || {
        let mut published = PublishedPlugins::publish(staged, &rollback_parent)?;
        let ledger_snapshot =
            match record_installations(&commit_plugins_dir, published.plugins(), scope) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    return match published.rollback() {
                        Ok(()) => Err(error),
                        Err(rollback_error) => Err(InstallError::RollbackFailed {
                            operation: format!("installation ledger update failed: {error}"),
                            reason: rollback_error.to_string(),
                        }),
                    };
                }
            };

        if let Some(dir) = commit_settings_dir
            && let Err(error) = write_enabled_plugins(&dir, &commit_closure)
        {
            let ledger_rollback = restore_ledger(&ledger_snapshot);
            let path_rollback = published.rollback();
            if ledger_rollback.is_err() || path_rollback.is_err() {
                return Err(InstallError::RollbackFailed {
                    operation: format!("settings activation failed: {error}"),
                    reason: format!(
                        "ledger: {}; plugin paths: {}",
                        ledger_rollback
                            .err()
                            .map(|error| error.to_string())
                            .unwrap_or_else(|| "restored".to_string()),
                        path_rollback
                            .err()
                            .map(|error| error.to_string())
                            .unwrap_or_else(|| "restored".to_string()),
                    ),
                });
            }
            return Err(InstallError::SettingsWriteFailed(error.to_string()));
        }

        published.commit();
        Ok::<_, InstallError>(())
    })
    .await
    .map_err(|error| {
        InstallError::Other(crate::PluginError::generic(
            "install",
            format!("plugin commit task failed: {error}"),
        ))
    })??;

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

#[derive(Debug)]
struct StagedPlugin {
    id: PluginId,
    entry: PluginMarketplaceEntry,
    stage_path: PathBuf,
    install_path: PathBuf,
    inspection: crate::artifact::ArtifactInspection,
}

#[derive(Debug)]
struct PublishedPath {
    install_path: PathBuf,
    backup_path: Option<PathBuf>,
    published_new: bool,
}

struct PublishedPlugins {
    plugins: Vec<StagedPlugin>,
    paths: Vec<PublishedPath>,
    committed: bool,
}

impl PublishedPlugins {
    fn publish(staged: Vec<StagedPlugin>, rollback_root: &Path) -> Result<Self, InstallError> {
        std::fs::create_dir_all(rollback_root).map_err(crate::PluginError::from)?;
        let mut published = Self {
            plugins: Vec::with_capacity(staged.len()),
            paths: Vec::with_capacity(staged.len()),
            committed: false,
        };

        for (index, plugin) in staged.into_iter().enumerate() {
            let operation = (|| -> crate::Result<()> {
                let parent = plugin.install_path.parent().ok_or_else(|| {
                    crate::PluginError::generic(
                        "install",
                        format!(
                            "install path has no parent: {}",
                            plugin.install_path.display()
                        ),
                    )
                })?;
                std::fs::create_dir_all(parent)?;
                let backup_path = match std::fs::symlink_metadata(&plugin.install_path) {
                    Ok(_) => {
                        let backup = rollback_root.join(index.to_string());
                        std::fs::rename(&plugin.install_path, &backup)?;
                        Some(backup)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error.into()),
                };
                published.paths.push(PublishedPath {
                    install_path: plugin.install_path.clone(),
                    backup_path,
                    published_new: false,
                });
                std::fs::rename(&plugin.stage_path, &plugin.install_path)?;
                if let Some(path) = published.paths.last_mut() {
                    path.published_new = true;
                }
                Ok(())
            })();
            if let Err(error) = operation {
                return match published.rollback() {
                    Ok(()) => Err(error.into()),
                    Err(rollback_error) => Err(InstallError::RollbackFailed {
                        operation: format!("publishing plugin '{}': {error}", plugin.id),
                        reason: rollback_error.to_string(),
                    }),
                };
            }
            published.plugins.push(plugin);
        }
        Ok(published)
    }

    fn plugins(&self) -> &[StagedPlugin] {
        &self.plugins
    }

    fn commit(&mut self) {
        self.committed = true;
        for path in &self.paths {
            if let Some(backup) = &path.backup_path
                && let Err(error) = remove_path(backup)
            {
                tracing::warn!(path = %backup.display(), %error, "failed to remove plugin rollback backup");
            }
        }
    }

    fn rollback(&mut self) -> std::io::Result<()> {
        if self.committed {
            return Ok(());
        }
        let mut failures = Vec::new();
        for path in self.paths.iter_mut().rev() {
            if path.published_new {
                match remove_path(&path.install_path) {
                    Ok(()) => path.published_new = false,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        path.published_new = false;
                    }
                    Err(error) => {
                        failures.push(format!("remove {}: {error}", path.install_path.display()));
                        continue;
                    }
                }
            }
            if let Some(backup) = path.backup_path.clone() {
                match std::fs::rename(&backup, &path.install_path) {
                    Ok(()) => path.backup_path = None,
                    Err(error) => failures.push(format!(
                        "restore {} from {}: {error}",
                        path.install_path.display(),
                        backup.display()
                    )),
                }
            }
        }
        if failures.is_empty() {
            self.committed = true;
            Ok(())
        } else {
            Err(std::io::Error::other(failures.join("; ")))
        }
    }
}

impl Drop for PublishedPlugins {
    fn drop(&mut self) {
        if let Err(error) = self.rollback() {
            tracing::error!(%error, "plugin install rollback failed during drop");
        }
    }
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

struct LedgerSnapshot {
    path: PathBuf,
    prior: Option<Vec<u8>>,
}

async fn acquire_install_lock(plugins_dir: &Path) -> Result<std::fs::File, InstallError> {
    let plugins_dir = plugins_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&plugins_dir)?;
        let path = plugins_dir.join(".install.lock");
        let mut options = std::fs::OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let file = options.open(&path)?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "plugin install lock `{}` is not a regular file",
                    path.display()
                ),
            ));
        }
        file.lock_exclusive()?;
        Ok::<_, std::io::Error>(file)
    })
    .await
    .map_err(|error| {
        InstallError::Other(crate::PluginError::generic(
            "install",
            format!("install lock task failed: {error}"),
        ))
    })?
    .map_err(|error| InstallError::Other(error.into()))
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
#[cfg(test)]
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
    coco_config::settings::writer::mutate_settings_file(&path, |value| {
        let map = value
            .as_object_mut()
            .ok_or_else(|| "settings.json is not a JSON object".to_string())?;
        let entries = map
            .entry("enabled_plugins".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        let entries_map = entries
            .as_object_mut()
            .ok_or_else(|| "enabled_plugins is not a JSON object".to_string())?;
        for id in closure {
            entries_map.insert(id.to_string(), serde_json::json!({ "enabled": true }));
        }
        Ok(())
    })
    .map_err(|error| std::io::Error::other(error.to_string()))
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
    coco_config::settings::writer::mutate_settings_file(&path, |value| {
        let map = value
            .as_object_mut()
            .ok_or_else(|| "settings.json is not a JSON object".to_string())?;
        let entries = map
            .entry("enabled_plugins".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        let entries_map = entries
            .as_object_mut()
            .ok_or_else(|| "enabled_plugins is not a JSON object".to_string())?;
        entries_map.insert(
            plugin_id.to_string(),
            serde_json::json!({ "enabled": enabled }),
        );
        Ok(())
    })
    .map_err(|error| std::io::Error::other(error.to_string()))
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

/// Atomically record the complete published closure in one ledger update.
fn record_installations(
    plugins_dir: &Path,
    plugins: &[StagedPlugin],
    scope: PluginScope,
) -> Result<LedgerSnapshot, InstallError> {
    const MAX_LEDGER_BYTES: usize = 8 * 1024 * 1024;
    let installed_path = plugins_dir.join("installed_plugins.json");
    let prior = match coco_utils_common::read_regular(&installed_path) {
        Ok(contents) if contents.len() <= MAX_LEDGER_BYTES => Some(contents),
        Ok(_) => {
            return Err(InstallError::Other(crate::PluginError::generic(
                "install",
                format!("installed plugin ledger exceeds {MAX_LEDGER_BYTES} bytes"),
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(InstallError::Other(error.into())),
    };
    let mut installed = InstalledPluginsManager::load(installed_path.clone())?;
    let now = Utc::now().to_rfc3339();
    for plugin in plugins {
        installed.record_installation(
            &plugin.id.to_string(),
            PluginInstallationEntry {
                scope,
                project_path: None,
                install_path: plugin.install_path.to_string_lossy().to_string(),
                version: plugin.entry.version.clone(),
                installed_at: Some(now.clone()),
                last_updated: Some(now.clone()),
                git_commit_sha: source_commit_sha(&plugin.entry),
                artifact_sha256: Some(plugin.inspection.tree_sha256.clone()),
                artifact_file_count: Some(plugin.inspection.file_count),
                artifact_total_bytes: Some(plugin.inspection.total_bytes),
                source: Some(plugin.entry.source.clone()),
            },
        );
    }
    installed.save()?;
    Ok(LedgerSnapshot {
        path: installed_path,
        prior,
    })
}

fn source_commit_sha(entry: &PluginMarketplaceEntry) -> Option<String> {
    use crate::schemas::RemotePluginSource;

    match &entry.source {
        crate::schemas::PluginSource::Remote(
            RemotePluginSource::Url { sha, .. }
            | RemotePluginSource::Github { sha, .. }
            | RemotePluginSource::GitSubdir { sha, .. },
        ) => sha.clone(),
        crate::schemas::PluginSource::Remote(
            RemotePluginSource::Npm { .. } | RemotePluginSource::Pip { .. },
        )
        | crate::schemas::PluginSource::RelativePath(_) => None,
    }
}

fn restore_ledger(snapshot: &LedgerSnapshot) -> std::io::Result<()> {
    match &snapshot.prior {
        Some(contents) => {
            coco_utils_common::replace_regular_atomic(&snapshot.path, contents).map(|_| ())
        }
        None => match std::fs::remove_file(&snapshot.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        },
    }
}

#[cfg(test)]
#[path = "install.test.rs"]
mod tests;
