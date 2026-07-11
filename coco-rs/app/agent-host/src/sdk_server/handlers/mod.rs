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
use tokio::sync::{Mutex, RwLock, mpsc};
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
mod session_state;
pub mod turn;
mod turn_runner_state;
mod turn_state;

use crate::session_runtime::ActiveTurnHandles;
use bootstrap_state::BootstrapState;
pub use dispatch::{HandlerContext, HandlerResult, SessionRequestContext, dispatch_client_request};
use runtime_replacement_state::RuntimeReplacementState;
use session_state::ScopedSessionState;
use turn_runner_state::TurnRunnerState;
use turn_state::TurnState;

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
    /// - `handoff`: the narrow subset of active session state the runner needs
    /// (id, shared history, live app state). Stats and other per-session
    /// state deliberately stay on the server-side slot to avoid an
    /// O (history) deep clone per turn.
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
        handoff: TurnHandoff,
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
        _handoff: TurnHandoff,
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

/// Narrow per-turn view of an active session handed to a [`TurnRunner`].
/// Holds only what the runner actually reads — session metadata used for
/// `QueryEngineConfig`, plus the `Arc`-wrapped shared history so the runner can
/// thread messages across turns. Crucially excludes `stats` and similar
/// server-bookkeeping state that was previously deep-cloned into the runner on
/// every turn.
#[derive(Debug, Clone)]
pub struct TurnHandoff {
    pub session_id: coco_types::SessionId,
    pub history: Arc<Mutex<Vec<std::sync::Arc<coco_messages::Message>>>>,
    /// Session-scoped shared state. Attached to every turn's engine
    /// via `with_app_state` so plan-mode cadence + live permission
    /// mode propagate across turns AND mid-session mode toggles
    /// reach the engine. `appState` is session-lifetime.
    pub app_state: Arc<RwLock<coco_types::ToolAppState>>,
    /// SDK initialize-scoped `planModeInstructions`, copied onto the session.
    pub plan_mode_instructions: Option<String>,
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

/// Shared server state carried across ClientRequests within a single stdio
/// session. Session-local state is keyed by `SessionId`; unscoped legacy
/// handlers may address the sole installed handoff.
#[derive(Default)]
pub struct SdkServerState {
    /// Installed turn runner. Defaults to a fail-closed runner until startup
    /// injects the QueryEngine-backed production runner.
    turn_runner: TurnRunnerState,
    /// Per-session turn counters, aggregate accounting, and active-turn
    /// handles. Kept separate from scoped handoff state because turn identity,
    /// result accounting, cancellation, and task drain handles have different
    /// cleanup rules.
    turn_state: TurnState,
    /// Event-driven last-activity clock for turn/session state transitions.
    session_activity: coco_app_server::SessionActivityTracker,
    /// Scoped SDK session handoff, metadata, and plan-mode state keyed by
    /// `SessionId`. Legacy no-runtime handlers and AppServer-scoped requests
    /// both enter through `SdkServerState` methods below.
    scoped_sessions: ScopedSessionState,
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

    pub(super) async fn archive_scoped_session<F>(
        &self,
        session_id: &coco_types::SessionId,
        build_result: F,
    ) -> ArchivedSessionState
    where
        F: FnOnce(&coco_types::SessionId, &Self) -> coco_types::SessionResultParams,
    {
        let result = build_result(session_id, self);
        self.clear_turn_counter(session_id);
        self.clear_session_accounting(session_id);
        self.clear_session_handoff(session_id);
        self.clear_session_metadata(session_id);
        self.clear_session_plan_mode_instructions(session_id);
        ArchivedSessionState {
            result,
            active_turn: None,
        }
    }

    pub(super) fn next_turn_id(&self, session_id: &coco_types::SessionId) -> coco_types::TurnId {
        self.turn_state.next_turn_id(session_id)
    }

    pub(super) fn clear_turn_counter(&self, session_id: &coco_types::SessionId) {
        self.turn_state.clear_turn_counter(session_id);
    }

    pub(super) fn reset_session_accounting(&self, session_id: coco_types::SessionId) {
        self.turn_state.reset_accounting(session_id);
    }

    pub(super) fn clear_session_accounting(&self, session_id: &coco_types::SessionId) {
        self.turn_state.clear_accounting(session_id);
    }

    pub(super) fn session_accounting_snapshot(
        &self,
        session_id: &coco_types::SessionId,
    ) -> SessionAccounting {
        self.turn_state.accounting_snapshot(session_id)
    }

    pub(super) fn accumulate_session_result(
        &self,
        session_id: &coco_types::SessionId,
        params: &coco_types::SessionResultParams,
    ) {
        self.turn_state.accumulate_result(session_id, params);
    }

    pub(super) async fn clear_scoped_session_state(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Option<ActiveTurnHandles> {
        self.clear_turn_counter(session_id);
        self.clear_session_accounting(session_id);
        self.clear_session_handoff(session_id);
        self.clear_session_metadata(session_id);
        self.clear_session_plan_mode_instructions(session_id);
        None
    }

    pub(super) fn install_scoped_replacement_session_state(
        &self,
        replacement: ReplacementSessionState,
    ) {
        self.clear_session_handoff(&replacement.session_id);
        if replacement.prior_cleanup == PriorSessionCleanup::Full {
            self.clear_turn_counter(&replacement.session_id);
            self.clear_session_accounting(&replacement.session_id);
            self.clear_session_metadata(&replacement.session_id);
            self.clear_session_plan_mode_instructions(&replacement.session_id);
        }

        if replacement.reset_accounting {
            self.reset_session_accounting(replacement.session_id.clone());
        }
        self.set_session_handoff(replacement.session_id.clone(), replacement.handoff);
        self.set_session_metadata(replacement.session_id.clone(), replacement.metadata);
        self.set_session_plan_mode_instructions(
            replacement.session_id.clone(),
            replacement.plan_mode_instructions,
        );
    }

    pub(super) fn set_session_handoff(
        &self,
        session_id: coco_types::SessionId,
        handoff: SessionHandoffState,
    ) {
        self.scoped_sessions
            .set_handoff(session_id.clone(), handoff);
        self.session_activity.touch(session_id);
    }

    pub fn session_handoff_snapshot(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Option<SessionHandoffState> {
        self.scoped_sessions.handoff_snapshot(session_id)
    }

    pub(super) fn clear_session_handoff(&self, session_id: &coco_types::SessionId) {
        self.scoped_sessions.clear_handoff(session_id);
        self.session_activity.forget(session_id);
    }

    pub(super) fn set_session_metadata(
        &self,
        session_id: coco_types::SessionId,
        metadata: SessionMetadata,
    ) {
        self.scoped_sessions.set_metadata(session_id, metadata);
    }

    pub(super) fn session_metadata_snapshot(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Option<SessionMetadata> {
        self.scoped_sessions.metadata_snapshot(session_id)
    }

    pub(super) fn update_session_model(
        &self,
        session_id: &coco_types::SessionId,
        model: String,
    ) -> Option<String> {
        self.scoped_sessions.update_model(session_id, model)
    }

    pub(super) fn clear_session_metadata(&self, session_id: &coco_types::SessionId) {
        self.scoped_sessions.clear_metadata(session_id);
    }

    pub(super) fn set_session_plan_mode_instructions(
        &self,
        session_id: coco_types::SessionId,
        instructions: Option<String>,
    ) {
        self.scoped_sessions
            .set_plan_mode_instructions(session_id, instructions);
    }

    pub(super) fn session_plan_mode_instructions(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Option<String> {
        self.scoped_sessions.plan_mode_instructions(session_id)
    }

    pub(super) fn clear_session_plan_mode_instructions(&self, session_id: &coco_types::SessionId) {
        self.scoped_sessions
            .clear_plan_mode_instructions(session_id);
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
            .field("session", &"RwLock<..>")
            .field("turn_runner", &"TurnRunnerState")
            .field("turn_state", &"TurnState")
            .field("scoped_sessions", &"ScopedSessionState")
            .field("bootstrap_state", &"BootstrapState")
            .field("runtime_replacement", &"RuntimeReplacementState")
            .field("session_store", &"SessionStore")
            .finish()
    }
}

/// State handed to SDK turns and local shortcuts for one active session.
#[derive(Clone)]
pub struct SessionHandoffState {
    /// Cumulative message history across every `turn/start` in this session.
    pub history: Arc<Mutex<Vec<std::sync::Arc<coco_messages::Message>>>>,
    /// Session-scoped `ToolAppState` attached to each turn's engine.
    pub app_state: Arc<RwLock<coco_types::ToolAppState>>,
}

impl SessionHandoffState {
    pub(super) fn new() -> Self {
        Self {
            history: Arc::new(Mutex::new(Vec::new())),
            app_state: Arc::new(RwLock::new(coco_types::ToolAppState::default())),
        }
    }
}

pub(super) struct ArchivedSessionState {
    pub result: coco_types::SessionResultParams,
    pub active_turn: Option<ActiveTurnHandles>,
}

pub(super) struct ActiveTurnStartState {
    pub session_id: coco_types::SessionId,
    pub turn_id: coco_types::TurnId,
    pub cancel_token: CancellationToken,
    pub handoff: SessionHandoffState,
    pub plan_mode_instructions: Option<String>,
}

pub(super) enum ActiveTurnStartError {
    NoActiveSession,
    TurnAlreadyRunning,
    MissingHandoff,
}

pub(super) struct ShortcutTurnState {
    pub session_id: coco_types::SessionId,
    pub turn_id: coco_types::TurnId,
    pub history: Arc<Mutex<Vec<std::sync::Arc<coco_messages::Message>>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PriorSessionCleanup {
    ActiveTurnAndHandoff,
    Full,
}

pub(super) struct ReplacementSessionState {
    pub session_id: coco_types::SessionId,
    pub metadata: SessionMetadata,
    pub handoff: SessionHandoffState,
    pub plan_mode_instructions: Option<String>,
    pub prior_cleanup: PriorSessionCleanup,
    pub reset_accounting: bool,
}

/// Legacy SDK metadata for one active session.
#[derive(Debug, Clone)]
pub(super) struct SessionMetadata {
    pub cwd: String,
    pub model: String,
}

/// Aggregate SDK accounting for one session.
#[derive(Debug, Clone)]
pub struct SessionAccounting {
    pub started_at: std::time::Instant,
    pub stats: SessionStats,
}

impl SessionAccounting {
    fn new() -> Self {
        Self {
            started_at: std::time::Instant::now(),
            stats: SessionStats::default(),
        }
    }
}

/// Aggregated per-session stats, mirrored from per-turn `SessionResult`
/// notifications emitted by `QueryEngine::run_with_events`.
/// Each field accumulates across every `turn/start` call in the session.
/// `session/archive` packages this into a single outbound `SessionResult`.
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub total_turns: i32,
    pub total_duration_api_ms: i64,
    pub total_cost_usd: f64,
    pub usage: coco_types::TokenUsage,
    pub model_usage: std::collections::HashMap<String, coco_types::SessionModelUsage>,
    pub permission_denials: Vec<coco_types::PermissionDenialInfo>,
    pub last_result_text: Option<String>,
    pub last_stop_reason: Option<String>,
    pub structured_output: Option<serde_json::Value>,
    pub had_error: bool,
    pub errors: Vec<String>,
    pub num_api_calls: i32,
}

impl SessionStats {
    fn accumulate(&mut self, params: &coco_types::SessionResultParams) {
        self.total_turns = self.total_turns.saturating_add(1);
        self.total_duration_api_ms = self
            .total_duration_api_ms
            .saturating_add(params.duration_api_ms);
        self.total_cost_usd += params.total_cost_usd;
        self.usage += params.usage;
        for (model, mu) in &params.model_usage {
            let entry = self.model_usage.entry(model.clone()).or_default();
            entry.input_tokens = entry.input_tokens.saturating_add(mu.input_tokens);
            entry.output_tokens = entry.output_tokens.saturating_add(mu.output_tokens);
            entry.cache_read_input_tokens = entry
                .cache_read_input_tokens
                .saturating_add(mu.cache_read_input_tokens);
            entry.cache_creation_input_tokens = entry
                .cache_creation_input_tokens
                .saturating_add(mu.cache_creation_input_tokens);
            entry.web_search_requests = entry
                .web_search_requests
                .saturating_add(mu.web_search_requests);
            entry.cost_usd += mu.cost_usd;
        }
        self.permission_denials
            .extend(params.permission_denials.iter().cloned());
        if params.result.is_some() {
            self.last_result_text = params.result.clone();
        }
        if params.structured_output.is_some() {
            self.structured_output = params.structured_output.clone();
        }
        self.last_stop_reason = Some(params.stop_reason.clone());
        if params.is_error {
            self.had_error = true;
            self.errors.extend(params.errors.iter().cloned());
        }
        if let Some(n) = params.num_api_calls {
            self.num_api_calls = self.num_api_calls.saturating_add(n);
        }
    }
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
