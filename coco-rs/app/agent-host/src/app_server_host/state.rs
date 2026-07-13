use std::{path::PathBuf, sync::Arc};

use crate::app_server_host::{HandlerResult, InitializeBootstrap};

use super::bootstrap_state::BootstrapState;
use super::runtime_replacement::{RuntimeReplacementContext, RuntimeReplacementState};
use super::session_store::SessionStore;
use super::turn_runner::{TurnRunner, TurnRunnerState};

/// Process-level AppServer host services shared across remote and local
/// connections. Live mutable session state belongs to the selected
/// `SessionRuntime`, never this owner.
#[derive(Default)]
pub struct AppServerHostState {
    /// Installed turn runner. Defaults to a fail-closed runner until startup
    /// injects the QueryEngine-backed production runner.
    turn_runner: TurnRunnerState,
    /// Event-driven last-activity clock for turn/session state transitions.
    session_activity: coco_app_server::SessionActivityTracker,
    /// Session transcript store installed by runtime startup.
    session_store: SessionStore,
    /// Pre-runtime initialize metadata, startup cwd, agent-summary opt-in,
    /// and startup-authorized bypass capability state.
    bootstrap_state: BootstrapState,
    /// Optional production runtime replacement context used by AppServer
    /// bridge `session/start` / `session/resume`.
    runtime_replacement: RuntimeReplacementState,
    /// Process-shared durable `session_seq` allocator.
    session_seq: Arc<coco_app_server::SessionSeqAllocator>,
}

impl AppServerHostState {
    pub(crate) async fn process_cwd(&self) -> Result<PathBuf, HandlerResult> {
        self.bootstrap_state
            .bootstrap_or_startup_cwd()
            .await
            .ok_or_else(|| HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "process workspace cwd is not configured".to_string(),
                data: None,
            })
    }

    pub fn session_seq_allocator(&self) -> &Arc<coco_app_server::SessionSeqAllocator> {
        &self.session_seq
    }

    pub(crate) fn session_last_activity(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Option<std::time::Instant> {
        self.session_activity.last_activity(session_id)
    }

    pub(crate) fn subscribe_session_activity(&self) -> tokio::sync::watch::Receiver<u64> {
        self.session_activity.subscribe()
    }

    pub async fn install_turn_runner(&self, runner: Arc<dyn TurnRunner>) {
        self.turn_runner.install(runner).await;
    }

    pub(crate) async fn turn_runner_snapshot(&self) -> Arc<dyn TurnRunner> {
        self.turn_runner.snapshot().await
    }

    pub async fn install_runtime_replacement(&self, context: RuntimeReplacementContext) {
        self.runtime_replacement.install(context).await;
    }

    pub(crate) async fn runtime_replacement_snapshot(&self) -> Option<RuntimeReplacementContext> {
        self.runtime_replacement.snapshot().await
    }

    pub(crate) fn forget_session_activity(&self, session_id: &coco_types::SessionId) {
        self.session_activity.forget(session_id);
    }

    pub(crate) fn touch_session_activity(&self, session_id: coco_types::SessionId) {
        self.session_activity.touch(session_id);
    }

    pub fn install_initialize_bootstrap_for_startup(
        &self,
        bootstrap: Arc<dyn InitializeBootstrap>,
    ) {
        self.bootstrap_state
            .install_initialize_bootstrap_for_startup(bootstrap);
    }

    pub fn install_startup_cwd(&self, cwd: PathBuf) {
        self.bootstrap_state.install_startup_cwd(cwd);
    }

    pub(crate) async fn initialize_bootstrap_snapshot(
        &self,
    ) -> Option<Arc<dyn InitializeBootstrap>> {
        self.bootstrap_state.initialize_bootstrap_snapshot().await
    }

    pub fn set_bypass_permissions_available(&self, available: bool) {
        self.bootstrap_state
            .set_bypass_permissions_available(available);
    }

    pub fn bypass_permissions_available(&self) -> bool {
        self.bootstrap_state.bypass_permissions_available()
    }

    pub(crate) async fn workspace_cwd(&self) -> Result<PathBuf, HandlerResult> {
        if let Some(cwd) = self.bootstrap_state.bootstrap_or_startup_cwd().await {
            return Ok(cwd);
        }
        Err(HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "workspace cwd is unavailable before session/start; provide session/start.cwd or install startup cwd".to_string(),
            data: None,
        })
    }

    pub fn install_session_manager_for_startup(&self, manager: Arc<coco_session::SessionManager>) {
        self.session_store.install_for_startup(manager);
    }

    pub async fn install_session_manager(&self, manager: Arc<coco_session::SessionManager>) {
        self.session_store.install(manager).await;
    }

    pub async fn session_manager_snapshot(&self) -> Option<Arc<coco_session::SessionManager>> {
        self.session_store.snapshot().await
    }
}

impl std::fmt::Debug for AppServerHostState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppServerHostState")
            .field("turn_runner", &"TurnRunnerState")
            .field("bootstrap_state", &"BootstrapState")
            .field("runtime_replacement", &"RuntimeReplacementState")
            .field("session_store", &"SessionStore")
            .finish()
    }
}
