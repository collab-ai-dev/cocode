use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::sync::RwLock;

use super::InitializeBootstrap;

#[derive(Default)]
pub(crate) struct BootstrapState {
    initialize_bootstrap: RwLock<Option<Arc<dyn InitializeBootstrap>>>,
    startup_cwd: RwLock<Option<PathBuf>>,
    bypass_permissions_available: AtomicBool,
}

impl BootstrapState {
    pub(crate) fn install_initialize_bootstrap_for_startup(
        &self,
        bootstrap: Arc<dyn InitializeBootstrap>,
    ) {
        let Ok(mut slot) = self.initialize_bootstrap.try_write() else {
            panic!(
                "install_initialize_bootstrap_for_startup: state was already locked at construction time"
            );
        };
        *slot = Some(bootstrap);
    }

    pub(crate) fn install_startup_cwd(&self, cwd: PathBuf) {
        let Ok(mut slot) = self.startup_cwd.try_write() else {
            panic!("install_startup_cwd: state was already locked at construction time");
        };
        *slot = Some(cwd);
    }

    pub(crate) async fn initialize_bootstrap_snapshot(
        &self,
    ) -> Option<Arc<dyn InitializeBootstrap>> {
        self.initialize_bootstrap.read().await.clone()
    }

    pub(crate) async fn bootstrap_or_startup_cwd(&self) -> Option<PathBuf> {
        if let Some(bootstrap) = self.initialize_bootstrap.read().await.as_ref() {
            return Some(bootstrap.cwd().await);
        }
        self.startup_cwd.read().await.clone()
    }

    pub(crate) fn set_bypass_permissions_available(&self, available: bool) {
        self.bypass_permissions_available
            .store(available, Ordering::Relaxed);
    }

    pub(crate) fn bypass_permissions_available(&self) -> bool {
        self.bypass_permissions_available.load(Ordering::Relaxed)
    }
}
