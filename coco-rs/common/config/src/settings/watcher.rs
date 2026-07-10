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

use super::SettingsRoots;
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
    /// developer's `config home/`).
    pub fn new(cwd: &Path) -> Self {
        Self::with_catalogs(cwd, &crate::runtime::CatalogPaths::default())
    }

    /// Create a watcher with explicit user / managed / catalog
    /// paths. Tests pass a TempDir-rooted `CatalogPaths` so the watch
    /// list reflects the isolated filesystem state, not the
    /// developer's real `config home/`.
    pub fn with_catalogs(cwd: &Path, catalogs: &crate::runtime::CatalogPaths) -> Self {
        Self::with_roots(&SettingsRoots::from_cwd(cwd), catalogs)
    }

    /// Create a watcher with explicit roots for project and local settings.
    pub fn with_roots(roots: &SettingsRoots, catalogs: &crate::runtime::CatalogPaths) -> Self {
        let watched_paths = vec![
            (
                WatchedKind::Settings(SettingSource::User),
                catalogs.user_settings.clone(),
            ),
            (
                WatchedKind::Settings(SettingSource::Project),
                crate::global_config::project_settings_path(roots.project_root()),
            ),
            (
                WatchedKind::Settings(SettingSource::Local),
                crate::global_config::local_settings_path(roots.local_root()),
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

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn with_roots_watches_project_and_local_settings_from_distinct_roots() {
        let tmp = TempDir::new().expect("tempdir");
        let project_root = tmp.path().join("project");
        let local_root = project_root.join("nested/session");
        let catalogs = crate::runtime::CatalogPaths::rooted(tmp.path().join("home"));
        let roots = SettingsRoots::new(&project_root, &local_root);
        let expected_project = project_root
            .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("settings.json");
        let expected_local = local_root
            .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("settings.local.json");

        let watcher = SettingsWatcher::with_roots(&roots, &catalogs);

        assert_eq!(
            watcher
                .watched_paths()
                .iter()
                .find(|(kind, _)| *kind == WatchedKind::Settings(SettingSource::Project))
                .map(|(_, path)| path),
            Some(&expected_project)
        );
        assert_eq!(
            watcher
                .watched_paths()
                .iter()
                .find(|(kind, _)| *kind == WatchedKind::Settings(SettingSource::Local))
                .map(|(_, path)| path),
            Some(&expected_local)
        );
    }
}
