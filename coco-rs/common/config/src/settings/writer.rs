//! Local-tier `settings.local.json` writer.
//!
//! # Why this exists
//!
//! Until the `/skills` editor in PR3, every coco-rs settings write
//! happened via a manual `vi config home/settings.json`. The TUI had no
//! direct path to persist a user choice — `/model` and `/permissions`
//! mutated session state only. The 2.1.142 `/skills` dialog needs a
//! synchronous write to `project config dir/settings.local.json` plus an
//! immediate `RuntimeConfig` rebuild so the next agent turn sees the
//! new state.
//!
//! # Wire shape
//!
//! [`write_local_settings`] takes a [`serde_json::Value`] patch
//! and deep-merges it into the on-disk JSON. `Value::Null` in the
//! patch is the **delete sentinel**: writing
//! `{"skill_overrides": {"foo": null}}` drops the `foo` key rather
//! than persisting a literal null.
//!
//! # Atomicity
//!
//! Writes go through a temp-file + rename so a crashed write never
//! leaves the file empty. The rebuild-publish call is synchronous —
//! the watcher's debounce window cannot leak a stale `RuntimeConfig`
//! to the next turn.

use std::fs;
use std::fs::OpenOptions;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;

use serde_json::Value;
use thiserror::Error;

use crate::env::EnvSnapshot;
use crate::runtime::CatalogPaths;
use crate::runtime::RuntimeConfig;
use crate::runtime::RuntimePublisher;
use crate::runtime::build_runtime_config_with;
use crate::settings::SettingsRoots;
use crate::settings::load_settings_with_roots_overriding_path;

/// Settings-write side errors. Boundary crate (`coco-config`) uses
/// `thiserror` per the error policy; main-trunk callers wrap via
/// `boxed`.
#[derive(Debug, Error)]
pub enum SettingsWriteError {
    #[error("io error reading or writing {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed json in {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid settings mutation: {message}")]
    Mutation { message: String },
    #[error("could not rebuild RuntimeConfig after write: {source}")]
    Rebuild {
        #[source]
        source: Box<crate::error::ConfigError>,
    },
}

static SETTINGS_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub struct RuntimePublishTarget {
    pub roots: SettingsRoots,
    pub publisher: Arc<RuntimePublisher>,
}

/// Deep-merge `patch` into `project config dir/settings.local.json`,
/// then rebuild + publish `RuntimeConfig` so the next agent turn
/// reads the new value without waiting for the file watcher.
/// Guarantees:
/// - **Atomic** — partial writes never corrupt the destination
/// (temp-file + rename pattern).
/// - **Delete sentinel** — `Value::Null` in the patch removes the
/// key instead of persisting a literal null (`B6` parity).
/// - **Synchronous publish** — when this call returns `Ok`,
/// subscribers to [`RuntimePublisher`] have seen the new
/// snapshot.
/// File IO and rebuild are sync; this function offloads to
/// `tokio::task::spawn_blocking` so the async caller (e.g. the TUI
/// dialog handler) doesn't stall the runtime.
pub async fn write_local_settings(
    cwd: impl Into<PathBuf>,
    flag_settings: Option<PathBuf>,
    catalogs: CatalogPaths,
    publisher: Arc<RuntimePublisher>,
    patch: Value,
) -> Result<(), SettingsWriteError> {
    write_local_settings_with_roots(
        SettingsRoots::from_cwd(cwd),
        flag_settings,
        catalogs,
        publisher,
        patch,
    )
    .await
}

/// Like [`write_local_settings`], but reloads project and local settings from
/// explicit roots after writing the local file.
pub async fn write_local_settings_with_roots(
    roots: SettingsRoots,
    flag_settings: Option<PathBuf>,
    catalogs: CatalogPaths,
    publisher: Arc<RuntimePublisher>,
    patch: Value,
) -> Result<(), SettingsWriteError> {
    let path = crate::global_config::local_settings_path(roots.local_root());
    tokio::task::spawn_blocking(move || {
        mutate_settings_and_republish(
            &path,
            move |current| {
                deep_merge_with_deletions(current, &patch);
                Ok(())
            },
            flag_settings.as_deref(),
            &catalogs,
            vec![RuntimePublishTarget { roots, publisher }],
        )
    })
    .await
    .map_err(|e| SettingsWriteError::Io {
        path: PathBuf::new(),
        source: std::io::Error::other(e.to_string()),
    })?
}

/// Read + deep-merge + atomic write. `Value::Null` in the overlay
/// removes the key (TS B6 parity).
#[cfg(test)]
fn apply_patch(path: &Path, patch: &Value) -> Result<(), SettingsWriteError> {
    mutate_settings_file(path, |current| {
        deep_merge_with_deletions(current, patch);
        Ok(())
    })
}

/// Serialize one read/modify/write transaction against a settings file.
/// Existing JSONC is accepted. A stable sibling lock coordinates coco
/// processes, while the process mutex coordinates threads; replacement then
/// uses the canonical sibling-tempfile writer.
#[cfg(test)]
pub(crate) fn mutate_settings_file(
    path: &Path,
    mutate: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<(), SettingsWriteError> {
    let _guard = SETTINGS_WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _file_guard = acquire_settings_write_lock(path)?;
    let mut current = read_or_default(path)?;
    mutate(&mut current).map_err(|message| SettingsWriteError::Mutation { message })?;
    validate_settings_value(&current)?;
    atomic_write(path, &current)
}

/// One ordered settings transaction: mutate in memory, validate every affected
/// runtime, then commit and publish before releasing the process/file guards.
/// Watcher rebuilds use publisher revisions, so a stale watcher snapshot
/// cannot overwrite a newer synchronous commit.
pub fn mutate_settings_and_republish(
    path: &Path,
    mutate: impl FnOnce(&mut Value) -> Result<(), String>,
    flag: Option<&Path>,
    catalogs: &CatalogPaths,
    targets: Vec<RuntimePublishTarget>,
) -> Result<(), SettingsWriteError> {
    let _guard = SETTINGS_WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _file_guard = acquire_settings_write_lock(path)?;
    let mut current = read_or_default(path)?;
    mutate(&mut current).map_err(|message| SettingsWriteError::Mutation { message })?;
    validate_settings_value(&current)?;

    let mut rebuilt = Vec::with_capacity(targets.len());
    for target in targets {
        rebuilt.push(rebuild_runtime_with_proposed_settings(
            path, &current, &target, flag, catalogs,
        )?);
    }

    atomic_write(path, &current)?;
    for (publisher, config) in rebuilt {
        let revision = publisher.reserve_revision();
        let _ = publisher.publish_reserved(revision, config);
    }
    Ok(())
}

fn acquire_settings_write_lock(path: &Path) -> Result<fs::File, SettingsWriteError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| SettingsWriteError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("settings.json");
    let lock_path = parent.join(format!(".{file_name}.write.lock"));
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|source| SettingsWriteError::Io {
            path: lock_path.clone(),
            source,
        })?;
    fs2::FileExt::lock_exclusive(&file).map_err(|source| SettingsWriteError::Io {
        path: lock_path,
        source,
    })?;
    Ok(file)
}

fn validate_settings_value(value: &Value) -> Result<(), SettingsWriteError> {
    let body = serde_json::to_string(value).map_err(|error| SettingsWriteError::Mutation {
        message: format!("failed to serialize settings: {error}"),
    })?;
    crate::settings::parse_settings(&body)
        .map(|_| ())
        .map_err(|error| SettingsWriteError::Mutation {
            message: error.to_string(),
        })
}

fn read_or_default(path: &Path) -> Result<Value, SettingsWriteError> {
    match fs::read_to_string(path) {
        Ok(contents) if contents.trim().is_empty() => Ok(Value::Object(Default::default())),
        Ok(contents) => {
            crate::jsonc::parse_value(&contents).map_err(|e| SettingsWriteError::Parse {
                path: path.to_path_buf(),
                source: serde_json::Error::io(std::io::Error::other(e.to_string())),
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Default::default())),
        Err(source) => Err(SettingsWriteError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Deep-merge with the `B6` deletion sentinel: a leaf `Value::Null`
/// in `overlay` removes the matching key from `base` (and recursively
/// prunes empty parent objects).
/// Differs from [`crate::settings::merge::deep_merge`] which preserves
/// nulls. We need the delete semantic for `skill_overrides` diff-
/// against-baseline writes.
fn deep_merge_with_deletions(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                if overlay_val.is_null() {
                    base_map.remove(key);
                    continue;
                }
                let entry = base_map
                    .entry(key.clone())
                    .or_insert(Value::Object(Default::default()));
                deep_merge_with_deletions(entry, overlay_val);
                // Prune empty objects so cleared maps don't leave
                // `"skill_overrides": {}` artefacts behind.
                if let Value::Object(inner) = entry
                    && inner.is_empty()
                {
                    base_map.remove(key);
                }
            }
        }
        (slot, overlay) => {
            *slot = overlay.clone();
        }
    }
}

/// Write through the workspace's canonical sibling-tempfile helper. This keeps
/// replacement semantics and durability handling consistent across platforms.
fn atomic_write(path: &Path, value: &Value) -> Result<(), SettingsWriteError> {
    let body = serde_json::to_vec_pretty(value).map_err(|source| SettingsWriteError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    coco_utils_common::fs::write_atomic(path, body).map_err(|source| SettingsWriteError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Build an affected runtime against the proposed in-memory settings. No disk
/// or publisher state changes until every target has rebuilt successfully.
fn rebuild_runtime_with_proposed_settings(
    path: &Path,
    proposed: &Value,
    target: &RuntimePublishTarget,
    flag: Option<&Path>,
    catalogs: &CatalogPaths,
) -> Result<(Arc<RuntimePublisher>, Arc<RuntimeConfig>), SettingsWriteError> {
    let env = EnvSnapshot::from_current_process();
    // Preserve the originally-resolved `--setting-sources` set across the
    // rebuild so a settings write doesn't silently re-enable a layer the
    // operator disabled.
    let current = target.publisher.current();
    let enabled = current.enabled_setting_sources.clone();
    let settings = load_settings_with_roots_overriding_path(
        &target.roots,
        flag,
        &catalogs.user_settings,
        &catalogs.managed_settings,
        &enabled,
        path,
        proposed,
    )
    .map_err(|e| SettingsWriteError::Rebuild {
        source: Box::new(e),
    })?;
    let rebuilt = build_runtime_config_with(
        settings,
        env,
        current.overrides.clone(),
        catalogs.clone(),
        enabled,
    )
    .map_err(|e| SettingsWriteError::Rebuild {
        source: Box::new(e),
    })?;
    Ok((Arc::clone(&target.publisher), Arc::new(rebuilt)))
}

#[cfg(test)]
#[path = "writer.test.rs"]
mod tests;
