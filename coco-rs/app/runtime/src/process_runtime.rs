//! Process-scoped runtime owner.
//!
//! Owns process-lifetime managers and hands cheap project/session handles to
//! startup paths.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use crate::project_services::ProjectRegistry;
use crate::project_services::ProjectRegistryManager;
use crate::project_services::ProjectServices;

pub struct ProcessRuntime {
    project_registry: ProjectRegistryManager,
}

impl ProcessRuntime {
    pub fn global() -> Arc<Self> {
        static GLOBAL: OnceLock<Arc<ProcessRuntime>> = OnceLock::new();
        GLOBAL
            .get_or_init(|| Arc::new(Self::start_global()))
            .clone()
    }

    fn start_global() -> Self {
        Self {
            project_registry: ProjectRegistryManager::start_global(),
        }
    }

    /// Apply the resolved `server.project_services_idle_ttl_secs` after config
    /// resolution (which happens after `global()` is first taken). The idle
    /// eviction loop reads the TTL live, so a post-startup update takes effect
    /// on the next sweep. A non-positive value keeps the built-in default.
    pub fn set_project_services_idle_ttl(&self, idle_for: Duration) {
        if idle_for.as_secs() > 0 {
            self.project_registry.registry().set_idle_ttl(idle_for);
        }
    }

    pub fn start(
        registry: &'static ProjectRegistry,
        idle_for: Duration,
        sweep_interval: Duration,
    ) -> Self {
        Self {
            project_registry: ProjectRegistryManager::start(registry, idle_for, sweep_interval),
        }
    }

    pub fn project_services(
        &self,
        config_home: &Path,
        project_root: impl Into<PathBuf>,
    ) -> Arc<ProjectServices> {
        self.project_registry
            .registry()
            .get_or_load(config_home, project_root)
    }

    pub fn reload_project_services(
        &self,
        config_home: &Path,
        project_root: impl Into<PathBuf>,
    ) -> Arc<ProjectServices> {
        self.project_registry
            .registry()
            .reload(config_home, project_root)
    }
}

#[cfg(test)]
#[path = "process_runtime.test.rs"]
mod tests;
