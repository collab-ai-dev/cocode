//! Per-method handlers for the SDK server dispatch loop.
//!
//! Each `ClientRequest` variant is routed to a handler function that
//! returns a `HandlerResult`. Handlers have access to a `HandlerContext`
//! carrying the notification channel (for emitting progress events
//! mid-handler) and any per-session state.
//!
//! This module is the dispatch hub. Handlers live in topical submodules:
//!
//! - [`session`] — `initialize`, `session/*`, event forwarding + aggregation
//! - [`turn`] — `turn/*`, `*/resolve`, `cancelRequest`
//! - [`runtime`] — `setModel` / `setModelRole` / `setPermissionMode` /
//! `setThinking` / `setAgentColor` / `applyPermissionUpdate` /
//! `resetSessionPermissionRules` / `updateEnv` / `stopTask` /
//! `context/usage` / `plugin/reload` / `hook/reload` / `config/applyFlags`
//! - [`config`] — `config/read` + `config/value/write`
//! - [`mcp`] — `mcp/status` / `mcp/setServers` / `mcp/reconnect` / `mcp/toggle`
//! - [`rewind`] — `control/rewindFiles`
//!
//! The dispatch match in [`dispatch_client_request`] is exhaustive — adding
//! a new `ClientRequest` variant fails compilation here, forcing a handler
//! to be written.

use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use coco_types::CoreEvent;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::sdk_server::session_store::SessionStore;

mod bootstrap_state;
pub mod config;
mod dispatch;
pub mod mcp;
pub mod rewind;
pub mod runtime;
mod runtime_replacement_state;
pub mod session;
pub mod turn;
mod turn_runner_state;

use crate::session_runtime::ActiveTurnHandles;
use bootstrap_state::BootstrapState;
pub use dispatch::{HandlerContext, HandlerResult, SessionRequestContext, dispatch_client_request};
use runtime_replacement_state::RuntimeReplacementState;
use turn_runner_state::TurnRunnerState;

/// The SDK protocol version coco-rs speaks.
pub const PROTOCOL_VERSION: &str = "1.0";

/// Default model id reported by `initialize` and used when `session/start` /
/// `setModel` omit a model param.
pub const DEFAULT_SDK_MODEL: &str = "claude-opus-4-6";

/// Default fast-mode / secondary model id advertised by `initialize`.
pub const DEFAULT_SDK_FAST_MODEL: &str = "claude-sonnet-4-6";

// ---------------------------------------------------------------------------
// TurnRunner — abstracts over "how to run a turn"
// ---------------------------------------------------------------------------

/// Boxed future used by trait methods.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Abstraction over how the SDK server executes a single turn.
/// The sdk_server module doesn't depend on `coco-query`, so the dispatch
/// layer stays pure. In production, the CLI entry point wires a concrete
/// runner that spawns a `QueryEngine`. Tests inject mock runners that
/// emit scripted events.
pub trait TurnRunner: Send + Sync {
    /// Run a single turn.
    /// - `params`: the `turn/start` parameters from the client.
    /// - `turn_id`: the server-minted id returned by `turn/start`; lifecycle
    ///   events emitted by the runner must use the same id.
    /// - `session`: the validated live runtime selected by AppServer routing.
    /// - `event_tx`: the channel on which CoreEvents must be emitted.
    /// The dispatcher's notification forwarder reads from this channel
    /// and writes JsonRpc notifications to the transport.
    /// - `cancel`: cancellation token. `turn/interrupt` triggers this.
    /// Returning `Ok (())` signals a clean turn completion. Returning an
    /// error causes the server to emit a `turn/failed` notification (future)
    /// and log the error.
    #[allow(clippy::too_many_arguments)]
    fn run_turn<'a>(
        &'a self,
        session: crate::session_runtime::SessionHandle,
        app_server: Arc<coco_app_server::AppServer<crate::sdk_server::LocalAppSessionHandle>>,
        params: coco_types::TurnStartParams,
        turn_id: coco_types::TurnId,
        event_tx: mpsc::Sender<CoreEvent>,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
}

/// Default runner used when no runner is injected. Returns an error
/// indicating that the server was not configured with a real runner.
pub struct NotImplementedRunner;

impl TurnRunner for NotImplementedRunner {
    fn run_turn<'a>(
        &'a self,
        _session: crate::session_runtime::SessionHandle,
        _app_server: Arc<coco_app_server::AppServer<crate::sdk_server::LocalAppSessionHandle>>,
        _params: coco_types::TurnStartParams,
        _turn_id: coco_types::TurnId,
        _event_tx: mpsc::Sender<CoreEvent>,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async {
            anyhow::bail!(
                "SdkServer was constructed without a TurnRunner; \
                 call SdkServer::with_turn_runner() before run_app_server_connection()"
            )
        })
    }
}

// ---------------------------------------------------------------------------
// InitializeBootstrap — cross-subsystem data provider for `initialize`
// ---------------------------------------------------------------------------

/// Provides the data fields that `InitializeResult` advertises to SDK clients.
/// `InitializeResult` is a cross-cutting bundle pulling from 5+ subsystems
/// (commands, agents, auth/account, config, rate-limit state). Rather than
/// plumbing each source through `SdkServerState` as a separate field, the
/// server takes one trait object that encapsulates all of them. The concrete
/// impl lives in `coco-agent-host` where every source is already imported; tests
/// can substitute a mock.
/// All accessors are `async` so implementations can do blocking I/O (agent
/// markdown walks, auth resolution) without forcing every caller to move
/// to spawn_blocking at the trait boundary.
#[async_trait::async_trait]
pub trait InitializeBootstrap: Send + Sync {
    /// Currently-visible slash commands (hidden / feature-gated ones are
    /// filtered out). Empty if no registry is wired.
    async fn commands(&self) -> Vec<coco_types::SdkSlashCommand>;

    /// Available subagents (built-ins + user-defined from disk). Empty if
    /// no agent source is wired.
    async fn agents(&self) -> Vec<coco_types::SdkAgentInfo>;

    /// Account / auth info for the logged-in user. Returns `default()` if
    /// no auth source is wired.
    async fn account(&self) -> coco_types::SdkAccountInfo;

    /// Currently-selected output style. Returns `"default"` if no source
    /// is wired.
    async fn output_style(&self) -> String;

    /// All output styles the server knows about (built-ins + user-defined
    /// markdown files). Returns `["default"]` if no source is wired.
    async fn available_output_styles(&self) -> Vec<String>;

    /// Current fast-mode rate-limit state, if tracked. Returns `None` to
    /// signal "feature not enabled" or "unknown".
    async fn fast_mode_state(&self) -> Option<coco_types::FastModeState>;

    /// Workspace cwd captured before a session/runtime exists.
    async fn cwd(&self) -> std::path::PathBuf;
}

// ---------------------------------------------------------------------------
// Server + session state
// ---------------------------------------------------------------------------

/// Process-level SDK services shared across accepted connections. Live mutable
/// session state belongs to the selected `SessionRuntime`, never this owner.
#[derive(Default)]
pub struct SdkServerState {
    /// Installed turn runner. Defaults to a fail-closed runner until startup
    /// injects the QueryEngine-backed production runner.
    turn_runner: TurnRunnerState,
    /// Event-driven last-activity clock for turn/session state transitions.
    session_activity: coco_app_server::SessionActivityTracker,
    /// Session transcript store installed by SDK/runtime startup. Callers use
    /// the install/snapshot methods below so the remaining `coco-session`
    /// dependency stays behind this owner.
    session_store: SessionStore,
    /// Pre-runtime initialize metadata, startup cwd, SDK agent-summary opt-in,
    /// and startup-authorized bypass capability state.
    bootstrap_state: BootstrapState,
    /// Optional SDK production runtime replacement context used by AppServer
    /// bridge `session/start` / `session/resume`.
    runtime_replacement: RuntimeReplacementState,

    /// Process-shared durable `session_seq` allocator. Every durable-envelope producer — the local AppServer
    /// forwarder, sidecar forwarders, and the SDK Hub egress — draws from
    /// these counters so per-session seqs stay strictly monotonic across
    /// delivery paths, and watermark persistence + skip-ahead keep them
    /// monotonic across process epochs.
    session_seq: std::sync::Arc<coco_app_server::SessionSeqAllocator>,
}

impl SdkServerState {
    pub(super) async fn process_cwd(&self) -> Result<PathBuf, HandlerResult> {
        self.bootstrap_state
            .bootstrap_or_startup_cwd()
            .await
            .ok_or_else(|| HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "process workspace cwd is not configured".to_string(),
                data: None,
            })
    }

    pub(crate) fn session_seq_allocator(
        &self,
    ) -> &std::sync::Arc<coco_app_server::SessionSeqAllocator> {
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

    pub(crate) fn install_turn_runner_for_startup(&self, runner: Arc<dyn TurnRunner>) {
        self.turn_runner.install_for_startup(runner);
    }

    pub(crate) async fn install_turn_runner(&self, runner: Arc<dyn TurnRunner>) {
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

    pub(super) fn forget_session_activity(&self, session_id: &coco_types::SessionId) {
        self.session_activity.forget(session_id);
    }

    pub(super) fn touch_session_activity(&self, session_id: coco_types::SessionId) {
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

    pub(super) async fn initialize_bootstrap_snapshot(
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

    pub(super) async fn workspace_cwd(&self) -> Result<PathBuf, HandlerResult> {
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

impl std::fmt::Debug for SdkServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SdkServerState")
            .field("turn_runner", &"TurnRunnerState")
            .field("bootstrap_state", &"BootstrapState")
            .field("runtime_replacement", &"RuntimeReplacementState")
            .field("session_store", &"SessionStore")
            .finish()
    }
}

pub(super) struct ActiveTurnStartState {
    pub session_id: coco_types::SessionId,
    pub turn_id: coco_types::TurnId,
    pub cancel_token: CancellationToken,
}

pub(super) enum ActiveTurnStartError {
    NoActiveSession,
    TurnAlreadyRunning,
}

pub(super) struct ShortcutTurnState {
    pub session_id: coco_types::SessionId,
    pub turn_id: coco_types::TurnId,
    pub history: Arc<Mutex<coco_messages::MessageHistory>>,
}

#[derive(Clone)]
pub struct RuntimeReplacementContext {
    pub startup_session_id: coco_types::SessionId,
    pub runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    pub process_runtime: Arc<coco_app_runtime::ProcessRuntime>,
    pub cwd: PathBuf,
    pub requires_structured_output: bool,
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
