use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use tokio::sync::RwLock;

use super::InitializeBootstrap;

#[derive(Default)]
pub(super) struct BootstrapState {
    initialize_bootstrap: RwLock<Option<Arc<dyn InitializeBootstrap>>>,
    startup_cwd: RwLock<Option<PathBuf>>,
    agent_progress_summaries_enabled: AtomicBool,
    bypass_permissions_available: AtomicBool,
}

impl BootstrapState {
    pub(super) fn install_initialize_bootstrap_for_startup(
        &self,
        bootstrap: Arc<dyn InitializeBootstrap>,
    ) {
        let Ok(mut slot) = self.initialize_bootstrap.try_write() else {
            panic!("with_initialize_bootstrap: state was already locked at construction time");
        };
        *slot = Some(bootstrap);
    }

    pub(super) fn install_startup_cwd(&self, cwd: PathBuf) {
        let Ok(mut slot) = self.startup_cwd.try_write() else {
            panic!("with_startup_cwd: state was already locked at construction time");
        };
        *slot = Some(cwd);
    }

    pub(super) async fn initialize_bootstrap_snapshot(
        &self,
    ) -> Option<Arc<dyn InitializeBootstrap>> {
        self.initialize_bootstrap.read().await.clone()
    }

    pub(super) async fn bootstrap_or_startup_cwd(&self) -> Option<PathBuf> {
        if let Some(bootstrap) = self.initialize_bootstrap.read().await.as_ref() {
            return Some(bootstrap.cwd().await);
        }
        self.startup_cwd.read().await.clone()
    }

    pub(super) fn enable_agent_progress_summaries(&self) {
        self.agent_progress_summaries_enabled
            .store(true, Ordering::SeqCst);
    }

    pub(super) fn agent_progress_summaries_enabled(&self) -> bool {
        self.agent_progress_summaries_enabled.load(Ordering::SeqCst)
    }

    pub(super) fn set_bypass_permissions_available(&self, available: bool) {
        self.bypass_permissions_available
            .store(available, Ordering::Relaxed);
    }

    pub(super) fn bypass_permissions_available(&self) -> bool {
        self.bypass_permissions_available.load(Ordering::Relaxed)
    }
}
