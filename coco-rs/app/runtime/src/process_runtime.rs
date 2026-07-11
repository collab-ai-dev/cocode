//! Process-scoped runtime owner.
//!
//! Owns process-lifetime managers and hands cheap project/session handles to
//! startup paths.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::project_services::ProjectRegistry;
use crate::project_services::ProjectRegistryManager;
use crate::project_services::ProjectServices;

pub struct ProcessRuntime {
    project_registry: ProjectRegistryManager,
    idle_ttl_applied: AtomicBool,
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
            idle_ttl_applied: AtomicBool::new(false),
        }
    }

    /// Apply the resolved `server.project_services_idle_ttl_secs` exactly once,
    /// from the first (startup) config resolution. `server.*` is process-scoped
    /// policy: a later session's fold — which may carry a
    /// project-layer override — must NOT mutate this process-global knob, so
    /// subsequent calls are ignored (this removes the previous cross-project
    /// last-writer-wins bleed). Full process-layer-only resolution that ignores
    /// even the startup project's override remains a refinement. A non-positive
    /// value keeps the built-in default and does not consume the one-shot.
    pub fn set_project_services_idle_ttl(&self, idle_for: Duration) {
        if idle_for.as_secs() > 0
            && self
                .idle_ttl_applied
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
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
            idle_ttl_applied: AtomicBool::new(false),
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
