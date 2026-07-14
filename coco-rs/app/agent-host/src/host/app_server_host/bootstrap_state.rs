use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use super::InitializeBootstrap;

pub(crate) struct BootstrapState {
    initialize_bootstrap: Option<Arc<dyn InitializeBootstrap>>,
    startup_cwd: Option<PathBuf>,
    bypass_permissions_available: AtomicBool,
}

impl Default for BootstrapState {
    fn default() -> Self {
        Self::new(None, None, false)
    }
}

impl BootstrapState {
    pub(crate) fn new(
        initialize_bootstrap: Option<Arc<dyn InitializeBootstrap>>,
        startup_cwd: Option<PathBuf>,
        bypass_permissions_available: bool,
    ) -> Self {
        Self {
            initialize_bootstrap,
            startup_cwd,
            bypass_permissions_available: AtomicBool::new(bypass_permissions_available),
        }
    }

    pub(crate) async fn initialize_bootstrap_snapshot(
        &self,
    ) -> Option<Arc<dyn InitializeBootstrap>> {
        self.initialize_bootstrap.clone()
    }

    pub(crate) async fn bootstrap_or_startup_cwd(&self) -> Option<PathBuf> {
        if let Some(bootstrap) = self.initialize_bootstrap.as_ref() {
            return Some(bootstrap.cwd().await);
        }
        self.startup_cwd.clone()
    }

    pub(crate) fn set_bypass_permissions_available(&self, available: bool) {
        self.bypass_permissions_available
            .store(available, Ordering::Relaxed);
    }

    pub(crate) fn bypass_permissions_available(&self) -> bool {
        self.bypass_permissions_available.load(Ordering::Relaxed)
    }
}
