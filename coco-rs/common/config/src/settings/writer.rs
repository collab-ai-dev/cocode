//! Local-tier `settings.local.json` writer.
//!
//! # Why this exists
//!
//! Until the `/skills` editor in PR3, every coco-rs settings write
//! happened via a manual `vi ~/.coco/settings.json`. The TUI had no
//! direct path to persist a user choice — `/model` and `/permissions`
//! mutated session state only. The 2.1.142 `/skills` dialog needs a
//! synchronous write to `<cwd>/.coco/settings.local.json` plus an
//! immediate `RuntimeConfig` rebuild so the next agent turn sees the
//! new state.
//!
//! # Wire shape
//!
//! [`SettingsWriter::write_local`] takes a [`serde_json::Value`] patch
//! and deep-merges it into the on-disk JSON. `Value::Null` in the
//! patch is the **delete sentinel** — TS `B6(mergeIntoSettings)`
//! does the same: writing `{"skill_overrides": {"foo": null}}` drops
//! the `foo` key rather than persisting a literal null.
//!
//! # Atomicity
//!
//! Writes go through a temp-file + rename so a crashed write never
//! leaves the file empty. The rebuild-publish call is synchronous —
//! the watcher's debounce window cannot leak a stale `RuntimeConfig`
//! to the next turn.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use thiserror::Error;

use crate::env::EnvSnapshot;
use crate::overrides::RuntimeOverrides;
use crate::runtime::CatalogPaths;
use crate::runtime::RuntimePublisher;
use crate::runtime::build_runtime_config_with;
use crate::settings::load_settings_with;

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
    #[error("could not rebuild RuntimeConfig after write: {source}")]
    Rebuild {
        #[source]
        source: Box<crate::error::ConfigError>,
    },
}

/// Deep-merge `patch` into `<cwd>/.coco/settings.local.json`,
/// then rebuild + publish `RuntimeConfig` so the next agent turn
/// reads the new value without waiting for the file watcher.
///
/// Guarantees:
///
/// - **Atomic** — partial writes never corrupt the destination
///   (temp-file + rename pattern).
/// - **Delete sentinel** — `Value::Null` in the patch removes the
///   key instead of persisting a literal null (TS `B6` parity).
/// - **Synchronous publish** — when this call returns `Ok`,
///   subscribers to [`RuntimePublisher`] have seen the new
///   snapshot.
///
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
    let cwd: PathBuf = cwd.into();
    let path = crate::global_config::local_settings_path(&cwd);
    tokio::task::spawn_blocking(move || {
        apply_patch(&path, &patch)?;
        republish_runtime(&cwd, flag_settings.as_deref(), &catalogs, &publisher)
    })
    .await
    .map_err(|e| SettingsWriteError::Io {
        path: PathBuf::new(),
        source: std::io::Error::other(e.to_string()),
    })?
}

// Compat shim — kept as a thin wrapper because tests + a few callsite
// docs reference `LocalSettingsWriter::new(...).write_local(patch)`.
// New code should prefer the free function above. Will be removed
// when the few stragglers migrate.
//
// Left intentionally; **do not** add new trait methods or new impls.
// The single-impl trait is YAGNI — kill it once nothing references
// the type alias.

/// **Deprecated** — call [`write_local_settings`] directly. The
/// trait + struct were a planned testable abstraction whose mock
/// never materialized.
pub struct LocalSettingsWriter {
    cwd: PathBuf,
    flag_settings: Option<PathBuf>,
    catalogs: CatalogPaths,
    publisher: Arc<RuntimePublisher>,
}

impl LocalSettingsWriter {
    pub fn new(
        cwd: impl Into<PathBuf>,
        catalogs: CatalogPaths,
        publisher: Arc<RuntimePublisher>,
    ) -> Self {
        Self {
            cwd: cwd.into(),
            flag_settings: None,
            catalogs,
            publisher,
        }
    }

    pub fn with_flag_settings(mut self, flag: Option<PathBuf>) -> Self {
        self.flag_settings = flag;
        self
    }

    /// Forwards to [`write_local_settings`].
    pub async fn write_local(&self, patch: Value) -> Result<(), SettingsWriteError> {
        write_local_settings(
            self.cwd.clone(),
            self.flag_settings.clone(),
            self.catalogs.clone(),
            self.publisher.clone(),
            patch,
        )
        .await
    }
}

/// Read + deep-merge + atomic write. `Value::Null` in the overlay
/// removes the key (TS B6 parity).
fn apply_patch(path: &Path, patch: &Value) -> Result<(), SettingsWriteError> {
    let mut current = read_or_default(path)?;
    deep_merge_with_deletions(&mut current, patch);
    atomic_write(path, &current)
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

/// Deep-merge with the TS `B6` deletion sentinel: a leaf `Value::Null`
/// in `overlay` removes the matching key from `base` (and recursively
/// prunes empty parent objects).
///
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

/// Write to a sibling tempfile and `rename` into place. The rename is
/// the atomic step on POSIX (and on Windows for same-volume moves).
fn atomic_write(path: &Path, value: &Value) -> Result<(), SettingsWriteError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| SettingsWriteError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let body = serde_json::to_vec_pretty(value).map_err(|source| SettingsWriteError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    let tmp = path.with_extension("local.json.tmp");
    {
        let mut file = fs::File::create(&tmp).map_err(|source| SettingsWriteError::Io {
            path: tmp.clone(),
            source,
        })?;
        file.write_all(&body)
            .map_err(|source| SettingsWriteError::Io {
                path: tmp.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| SettingsWriteError::Io {
            path: tmp.clone(),
            source,
        })?;
    }
    fs::rename(&tmp, path).map_err(|source| SettingsWriteError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Rebuild `RuntimeConfig` from the on-disk settings + publish so the
/// next agent turn reads the fresh tiers. Synchronous so the dialog's
/// save handler can rely on the new state being visible before its
/// `AvailableCommandsRefreshed` push fires.
fn republish_runtime(
    cwd: &Path,
    flag: Option<&Path>,
    catalogs: &CatalogPaths,
    publisher: &RuntimePublisher,
) -> Result<(), SettingsWriteError> {
    let env = EnvSnapshot::from_current_process();
    // Preserve the originally-resolved `--setting-sources` set across the
    // rebuild so a settings write doesn't silently re-enable a layer the
    // operator disabled.
    let enabled = publisher.current().enabled_setting_sources.clone();
    let settings = load_settings_with(
        cwd,
        flag,
        &catalogs.user_settings,
        &catalogs.managed_settings,
        &enabled,
    )
    .map_err(|e| SettingsWriteError::Rebuild {
        source: Box::new(e),
    })?;
    let rebuilt = build_runtime_config_with(
        settings,
        env,
        RuntimeOverrides::default(),
        catalogs.clone(),
        enabled,
    )
    .map_err(|e| SettingsWriteError::Rebuild {
        source: Box::new(e),
    })?;
    publisher.publish(Arc::new(rebuilt));
    Ok(())
}

#[cfg(test)]
#[path = "writer.test.rs"]
mod tests;
