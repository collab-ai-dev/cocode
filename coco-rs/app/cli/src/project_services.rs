//! Project-scoped service/catalog preparation.
//!
//! This is the first narrow slice of the future `ProjectServices` container:
//! a per-project plugin catalog snapshot plus the registry that shares it
//! across sessions in the same process.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::RwLock;
use std::sync::RwLockReadGuard;
use std::sync::RwLockWriteGuard;

/// Process-wide project registry used until `ProcessRuntime` owns this field.
pub fn project_registry() -> &'static ProjectRegistry {
    static REGISTRY: OnceLock<ProjectRegistry> = OnceLock::new();
    REGISTRY.get_or_init(ProjectRegistry::default)
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ProjectRegistryKey {
    config_home: PathBuf,
    project_root: PathBuf,
}

impl ProjectRegistryKey {
    fn new(config_home: &Path, project_root: impl Into<PathBuf>) -> Self {
        Self {
            config_home: config_home.to_path_buf(),
            project_root: project_root.into(),
        }
    }
}

/// Cache of project-scoped service snapshots.
///
/// Loading is intentionally synchronous today, so holding the write lock during
/// a miss gives a simple single-flight guarantee: concurrent callers for the
/// same key share the first loaded `Arc<ProjectServices>`.
#[derive(Debug, Default)]
pub struct ProjectRegistry {
    projects: RwLock<HashMap<ProjectRegistryKey, Arc<ProjectServices>>>,
}

impl ProjectRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_load(
        &self,
        config_home: &Path,
        project_root: impl Into<PathBuf>,
    ) -> Arc<ProjectServices> {
        let key = ProjectRegistryKey::new(config_home, project_root);
        if let Some(project) = self.read_projects().get(&key) {
            return project.clone();
        }

        let mut projects = self.write_projects();
        projects
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(ProjectServices::load(
                    &key.config_home,
                    key.project_root.clone(),
                ))
            })
            .clone()
    }

    pub fn reload(
        &self,
        config_home: &Path,
        project_root: impl Into<PathBuf>,
    ) -> Arc<ProjectServices> {
        let key = ProjectRegistryKey::new(config_home, project_root);
        let project = Arc::new(ProjectServices::load(
            &key.config_home,
            key.project_root.clone(),
        ));
        self.write_projects().insert(key, project.clone());
        project
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.read_projects().len()
    }

    fn read_projects(
        &self,
    ) -> RwLockReadGuard<'_, HashMap<ProjectRegistryKey, Arc<ProjectServices>>> {
        match self.projects.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn write_projects(
        &self,
    ) -> RwLockWriteGuard<'_, HashMap<ProjectRegistryKey, Arc<ProjectServices>>> {
        match self.projects.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Project-scoped services for one resolved project root.
#[derive(Debug, Clone)]
pub struct ProjectServices {
    catalog: ProjectCatalogSnapshot,
}

impl ProjectServices {
    pub fn load(config_home: &Path, project_root: impl Into<PathBuf>) -> Self {
        Self {
            catalog: ProjectCatalogSnapshot::load(config_home, project_root),
        }
    }

    pub fn project_root(&self) -> &Path {
        self.catalog.project_root()
    }

    pub fn catalog(&self) -> &ProjectCatalogSnapshot {
        &self.catalog
    }

    pub fn plugins(&self) -> &[coco_plugins::loader::LoadedPluginV2] {
        self.catalog.plugins()
    }

    pub fn output_style_sources(&self) -> Vec<coco_output_styles::PluginOutputStyleSource> {
        self.catalog.output_style_sources()
    }

    pub fn agent_search_paths(
        &self,
        config_home: &Path,
        cwd: &Path,
    ) -> coco_subagent::definition_store::AgentSearchPaths {
        self.catalog.agent_search_paths(config_home, cwd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn registry_reuses_services_for_same_project_root() {
        let temp = tempdir().unwrap();
        let config_home = temp.path().join("home");
        let project_root = temp.path().join("repo");
        std::fs::create_dir_all(&config_home).unwrap();
        std::fs::create_dir_all(&project_root).unwrap();
        let registry = ProjectRegistry::new();

        let first = registry.get_or_load(&config_home, project_root.clone());
        let second = registry.get_or_load(&config_home, project_root.clone());

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(registry.len(), 1);
        assert_eq!(first.project_root(), project_root.as_path());
    }

    #[test]
    fn registry_separates_project_roots() {
        let temp = tempdir().unwrap();
        let config_home = temp.path().join("home");
        let project_a = temp.path().join("repo-a");
        let project_b = temp.path().join("repo-b");
        std::fs::create_dir_all(&config_home).unwrap();
        std::fs::create_dir_all(&project_a).unwrap();
        std::fs::create_dir_all(&project_b).unwrap();
        let registry = ProjectRegistry::new();

        let first = registry.get_or_load(&config_home, project_a);
        let second = registry.get_or_load(&config_home, project_b);

        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn registry_reload_replaces_cached_entry() {
        let temp = tempdir().unwrap();
        let config_home = temp.path().join("home");
        let project_root = temp.path().join("repo");
        std::fs::create_dir_all(&config_home).unwrap();
        std::fs::create_dir_all(&project_root).unwrap();
        let registry = ProjectRegistry::new();

        let first = registry.get_or_load(&config_home, project_root.clone());
        let second = registry.reload(&config_home, project_root.clone());
        let third = registry.get_or_load(&config_home, project_root);

        assert!(!Arc::ptr_eq(&first, &second));
        assert!(Arc::ptr_eq(&second, &third));
        assert_eq!(registry.len(), 1);
    }
}

/// Project-scoped plugin catalog loaded against a resolved project root.
#[derive(Debug, Clone)]
pub struct ProjectCatalogSnapshot {
    project_root: PathBuf,
    plugins: Vec<coco_plugins::loader::LoadedPluginV2>,
}

impl ProjectCatalogSnapshot {
    pub fn load(config_home: &Path, project_root: impl Into<PathBuf>) -> Self {
        let project_root = project_root.into();
        let plugins = coco_plugins::load_enabled_plugins(config_home, &project_root);
        Self {
            project_root,
            plugins,
        }
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn plugins(&self) -> &[coco_plugins::loader::LoadedPluginV2] {
        &self.plugins
    }

    pub fn output_style_sources(&self) -> Vec<coco_output_styles::PluginOutputStyleSource> {
        self.plugins
            .iter()
            .map(coco_output_styles::PluginOutputStyleSource::from_loaded_plugin)
            .collect()
    }

    pub fn agent_search_paths(
        &self,
        config_home: &Path,
        cwd: &Path,
    ) -> coco_subagent::definition_store::AgentSearchPaths {
        crate::paths::standard_agent_search_paths_with_plugins(config_home, cwd, &self.plugins)
    }
}
