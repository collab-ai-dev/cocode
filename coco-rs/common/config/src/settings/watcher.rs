//! Settings + catalog file watcher.
//!
//! Tracks four settings layers (user / project / local / policy) plus
//! the two sibling catalog files (`providers.json` / `models.json`).
//! A file change in any of the six paths triggers a fresh
//! `RuntimeConfig` build (see `multi-provider-plan.md` §11). The
//! actual debounce + dispatch wiring lives in
//! `coco-config-reload`; this struct describes WHAT to watch.

use std::path::Path;
use std::path::PathBuf;

use super::source::SettingSource;

/// Marks a watched path as either a settings layer (with its source)
/// or a sibling catalog file. Settings layers feed the per-source
/// merge; catalog files trigger a registry rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchedKind {
    Settings(SettingSource),
    ProvidersCatalog,
    ModelsCatalog,
}

/// Watcher description for the runtime-config build pipeline.
pub struct SettingsWatcher {
    watched_paths: Vec<(WatchedKind, PathBuf)>,
}

impl SettingsWatcher {
    /// Create a watcher using the default `CatalogPaths` (the
    /// developer's `~/.coco/`).
    pub fn new(cwd: &Path) -> Self {
        Self::with_catalogs(cwd, &crate::runtime::CatalogPaths::default())
    }

    /// Create a watcher with explicit user / managed / catalog
    /// paths. Tests pass a TempDir-rooted `CatalogPaths` so the watch
    /// list reflects the isolated filesystem state, not the
    /// developer's real `~/.coco/`.
    pub fn with_catalogs(cwd: &Path, catalogs: &crate::runtime::CatalogPaths) -> Self {
        let watched_paths = vec![
            (
                WatchedKind::Settings(SettingSource::User),
                catalogs.user_settings.clone(),
            ),
            (
                WatchedKind::Settings(SettingSource::Project),
                crate::global_config::project_settings_path(cwd),
            ),
            (
                WatchedKind::Settings(SettingSource::Local),
                crate::global_config::local_settings_path(cwd),
            ),
            (
                WatchedKind::Settings(SettingSource::Policy),
                catalogs.managed_settings.clone(),
            ),
            (WatchedKind::ProvidersCatalog, catalogs.providers.clone()),
            (WatchedKind::ModelsCatalog, catalogs.models.clone()),
        ];
        Self { watched_paths }
    }

    /// Get watched paths with their kind.
    pub fn watched_paths(&self) -> &[(WatchedKind, PathBuf)] {
        &self.watched_paths
    }

    /// Determine which kind a path belongs to.
    pub fn kind_for_path(&self, path: &Path) -> Option<WatchedKind> {
        self.watched_paths
            .iter()
            .find(|(_, p)| p == path)
            .map(|(k, _)| *k)
    }

    /// Determine which settings source a path belongs to (returns
    /// `None` for catalog files, which are not part of the settings
    /// merge).
    pub fn source_for_path(&self, path: &Path) -> Option<SettingSource> {
        self.kind_for_path(path).and_then(|k| match k {
            WatchedKind::Settings(source) => Some(source),
            _ => None,
        })
    }
}
