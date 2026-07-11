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

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use coco_app_server_transport::JsonRpcFrame;
use coco_app_server_transport::JsonRpcRequest as TransportJsonRpcRequest;
use coco_types::ApprovalResolveParams;
use coco_types::CoreEvent;
use coco_types::ElicitationResolveParams;
use coco_types::UserInputResolveParams;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::sdk_server::outbound::OutboundMessage;
use crate::sdk_server::pending_map::ResolveOutcome;
use crate::sdk_server::session_store::SessionStore;
use crate::sdk_server::transport::SdkTransport;

mod bootstrap_state;
pub mod config;
mod connection_state;
mod dispatch;
mod file_history_state;
mod initialize_state;
pub mod mcp;
mod mcp_manager_state;
mod mcp_registration_state;
mod pending_client_request_state;
pub mod rewind;
pub mod runtime;
mod runtime_reload_state;
mod runtime_replacement_state;
mod server_request_state;
pub mod session;
mod session_runtime_state;
mod session_state;
pub mod turn;
mod turn_runner_state;
mod turn_state;

use bootstrap_state::BootstrapState;
use connection_state::ConnectionState;
pub use dispatch::HandlerContext;
pub use dispatch::HandlerResult;
pub use dispatch::dispatch_client_request;
use file_history_state::FileHistoryStateSlot;
use initialize_state::InitializeState;
use mcp_manager_state::McpManagerState;
use mcp_registration_state::McpRegistrationState;
use mcp_registration_state::McpRegistrationStatusProjection;
use pending_client_request_state::PendingClientRequestState;
use runtime_reload_state::RuntimeReloadState;
use runtime_replacement_state::RuntimeReplacementState;
use server_request_state::ServerRequestState;
use session_runtime_state::SessionRuntimeState;
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

type SharedFileHistory = Arc<RwLock<coco_context::FileHistoryState>>;

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
    fn run_turn<'a>(
        &'a self,
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
    /// Pending client-resolved requests for approvals, user input, and MCP
    /// elicitations.
    pending_client_requests: PendingClientRequestState,
    /// Pending AppServer-frame-shaped ServerRequests plus issued-id counter.
    server_requests: ServerRequestState,
    /// SDK connection transport and ordered outbound queue. Transport is
    /// populated before startup and refreshed by the AppServer bridge; the
    /// outbound queue exists only while the bridge writer is running.
    connection_state: ConnectionState,
    /// Session transcript store installed by SDK/runtime startup. Callers use
    /// the install/snapshot methods below so the remaining `coco-session`
    /// dependency stays behind this owner.
    session_store: SessionStore,
    /// Optional file-history state plus config home used by
    /// `control/rewindFiles`.
    file_history: FileHistoryStateSlot,
    /// Optional MCP connection manager used by the `mcp/setServers`,
    /// `mcp/reconnect`, `mcp/toggle` handlers.
    mcp_manager: McpManagerState,
    /// Pre-runtime initialize metadata, startup cwd, SDK agent-summary opt-in,
    /// and startup-authorized bypass capability state.
    bootstrap_state: BootstrapState,
    /// Process-shared `SessionHandle`. Set by SDK startup and swapped by
    /// AppServer-backed SDK `session/start` / `session/resume` replacement
    /// paths. `None` only in tests that don't wire a runtime.
    session_runtime: SessionRuntimeState,
    /// Runtime-owned SDK reload subscriber for the currently installed
    /// `session_runtime`.
    runtime_reload: RuntimeReloadState,
    /// Optional SDK production runtime replacement context used by AppServer
    /// bridge `session/start` / `session/resume`.
    runtime_replacement: RuntimeReplacementState,

    /// Initialize-scoped SDK inputs replayed into new/replacement sessions:
    /// SDK-supplied agents, plan-mode instructions, and hook callbacks.
    initialize_state: InitializeState,

    /// Last MCP tool-registration reports used by `mcp/status`.
    mcp_registration: McpRegistrationState,

    /// Process-shared durable `session_seq` allocator. Every durable-envelope producer — the local AppServer
    /// forwarder, sidecar forwarders, and the SDK Hub egress — draws from
    /// these counters so per-session seqs stay strictly monotonic across
    /// delivery paths, and watermark persistence + skip-ahead keep them
    /// monotonic across process epochs.
    session_seq: std::sync::Arc<coco_app_server::SessionSeqAllocator>,
}

impl SdkServerState {
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

    pub(crate) fn install_session_runtime_for_startup(
        &self,
        runtime: crate::session_runtime::SessionHandle,
    ) {
        self.session_runtime.install_for_startup(runtime);
    }

    pub async fn install_session_runtime(&self, runtime: crate::session_runtime::SessionHandle) {
        self.session_runtime.install(runtime).await;
    }

    pub async fn session_runtime_snapshot(&self) -> Option<crate::session_runtime::SessionHandle> {
        self.session_runtime.snapshot().await
    }

    pub async fn has_session_runtime(&self) -> bool {
        self.session_runtime.is_installed().await
    }

    /// Clear the process-shared singletons (`session_runtime` / `turn_runner` /
    /// `mcp_manager` / `file_history`) when the closing id still matches the
    /// installed runtime. Without this, a close cascade
    /// clears the session-keyed maps but leaves these singletons installed, so a
    /// zombie session id keeps receiving stamps and "successful" control
    /// mutations against a shut-down runtime after archive / idle-sweep. The
    /// runtime match is compare-and-cleared atomically so a replacement swap
    /// that already installed a different runtime is never torn down.
    pub(super) async fn clear_installed_singletons_if_matches(
        &self,
        session_id: &coco_types::SessionId,
    ) {
        if !self.session_runtime.clear_if_matches(session_id).await {
            return;
        }
        self.turn_runner.clear().await;
        self.mcp_manager.clear().await;
        self.file_history.install(None, None).await;
        self.runtime_reload.abort_current().await;
    }

    pub(crate) fn install_sdk_transport_for_startup(&self, transport: Arc<dyn SdkTransport>) {
        self.connection_state
            .install_transport_for_startup(transport);
    }

    pub(crate) async fn install_sdk_transport(&self, transport: Arc<dyn SdkTransport>) {
        self.connection_state.install_transport(transport).await;
    }

    pub(crate) async fn sdk_transport_snapshot(&self) -> Option<Arc<dyn SdkTransport>> {
        self.connection_state.transport_snapshot().await
    }

    pub(crate) fn install_mcp_manager_for_startup(
        &self,
        manager: Arc<Mutex<coco_mcp::McpConnectionManager>>,
    ) {
        self.mcp_manager.install_for_startup(manager);
    }

    pub async fn install_mcp_manager(&self, manager: Arc<Mutex<coco_mcp::McpConnectionManager>>) {
        self.mcp_manager.install(manager).await;
    }

    pub async fn mcp_manager_snapshot(&self) -> Option<Arc<Mutex<coco_mcp::McpConnectionManager>>> {
        self.mcp_manager.snapshot().await
    }

    pub async fn install_runtime_replacement(&self, context: RuntimeReplacementContext) {
        self.runtime_replacement.install(context).await;
    }

    pub(crate) async fn runtime_replacement_snapshot(&self) -> Option<RuntimeReplacementContext> {
        self.runtime_replacement.snapshot().await
    }

    pub(crate) async fn abort_sdk_runtime_reload_subscription(&self) {
        self.runtime_reload.abort_current().await;
    }

    pub(crate) async fn install_sdk_runtime_reload_subscription(
        &self,
        handle: tokio::task::JoinHandle<()>,
    ) {
        self.runtime_reload.install(handle).await;
    }

    pub(crate) async fn install_sdk_outbound_tx(&self, tx: mpsc::Sender<OutboundMessage>) {
        self.connection_state.install_outbound_tx(tx).await;
    }

    pub(crate) async fn sdk_outbound_tx_snapshot(&self) -> Option<mpsc::Sender<OutboundMessage>> {
        self.connection_state.outbound_tx_snapshot().await
    }

    pub(crate) async fn clear_sdk_outbound_tx(&self) {
        self.connection_state.clear_outbound_tx().await;
    }

    #[cfg(test)]
    pub(crate) async fn has_sdk_outbound_tx(&self) -> bool {
        self.connection_state.has_outbound_tx().await
    }

    /// Best-effort current session id for process-level bridge fallbacks that
    /// do not have an AppServer request context.
    pub async fn runtime_or_active_session_id(&self) -> Option<coco_types::SessionId> {
        let runtime = self.session_runtime_snapshot().await;
        if let Some(runtime) = runtime {
            return Some(runtime.current_typed_session_id().await);
        }
        self.sole_session_handoff_id()
    }

    #[cfg(test)]
    pub(super) async fn install_test_session_state(
        &self,
        session_id: coco_types::SessionId,
        metadata: SessionMetadata,
    ) {
        self.set_session_metadata(session_id.clone(), metadata);
        self.set_session_handoff(session_id, SessionHandoffState::new());
    }

    pub(super) async fn claim_started_session_state(
        &self,
        started: StartedSessionState,
    ) -> Result<(), StartedSessionState> {
        if self.has_session_handoffs() {
            return Err(started);
        }
        self.set_session_metadata(started.session_id.clone(), started.metadata);
        self.set_session_plan_mode_instructions(
            started.session_id.clone(),
            started.plan_mode_instructions.clone(),
        );
        self.set_session_handoff(started.session_id.clone(), started.handoff);
        self.reset_session_accounting(started.session_id);
        Ok(())
    }

    pub(super) async fn archive_active_session<F>(
        &self,
        requested_session_id: &coco_types::SessionId,
        build_result: F,
    ) -> Result<ArchivedSessionState, ArchiveSessionError>
    where
        F: FnOnce(&coco_types::SessionId, &Self) -> coco_types::SessionResultParams,
    {
        let Some(active_session_id) = self.sole_session_handoff_id() else {
            return Err(ArchiveSessionError::NoActiveSession);
        };
        if active_session_id != *requested_session_id {
            return Err(ArchiveSessionError::SessionMismatch {
                active: active_session_id,
                requested: requested_session_id.clone(),
            });
        }
        let result = build_result(&active_session_id, self);
        let active_turn = self.take_active_turn(&active_session_id);
        self.clear_turn_counter(&active_session_id);
        self.clear_session_accounting(&active_session_id);
        self.clear_session_handoff(&active_session_id);
        self.clear_session_metadata(&active_session_id);
        self.clear_session_plan_mode_instructions(&active_session_id);
        Ok(ArchivedSessionState {
            result,
            active_turn,
        })
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
        let active_turn = self.take_active_turn(session_id);
        self.clear_turn_counter(session_id);
        self.clear_session_accounting(session_id);
        self.clear_session_handoff(session_id);
        self.clear_session_metadata(session_id);
        self.clear_session_plan_mode_instructions(session_id);
        ArchivedSessionState {
            result,
            active_turn,
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

    pub(super) fn has_active_turn(&self, session_id: &coco_types::SessionId) -> bool {
        self.turn_state.has_active_turn(session_id)
    }

    pub(super) fn active_turn_cancel_token(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Option<CancellationToken> {
        self.turn_state.active_turn_cancel_token(session_id)
    }

    pub(super) fn mint_shortcut_turn_for_session(
        &self,
        session_id: coco_types::SessionId,
    ) -> Result<ShortcutTurnState, ActiveTurnStartError> {
        if self.has_active_turn(&session_id) {
            return Err(ActiveTurnStartError::TurnAlreadyRunning);
        }
        let turn_id = self.next_turn_id(&session_id);
        let Some(handoff) = self.session_handoff_snapshot(&session_id) else {
            return Err(ActiveTurnStartError::MissingHandoff);
        };
        let shortcut = ShortcutTurnState {
            session_id,
            turn_id,
            history: handoff.history,
        };
        self.session_activity.touch(shortcut.session_id.clone());
        Ok(shortcut)
    }

    pub(super) fn start_active_turn_for_session<F>(
        &self,
        session_id: coco_types::SessionId,
        build_handles: F,
    ) -> Result<coco_types::TurnId, ActiveTurnStartError>
    where
        F: FnOnce(ActiveTurnStartState) -> ActiveTurnHandles,
    {
        // check-occupied + mint + install run in ONE critical section over
        // the per-session turn record. Splitting them (as before) let two
        // connections both pass the occupancy check and the second install leak
        // the first turn's cancel token — two uninterruptible concurrent turns
        // on one engine, matching per-session turn serialization.
        let activity_session_id = session_id.clone();
        let result = self.turn_state.start_active_turn(&session_id, || {
            let turn_id = self.next_turn_id(&session_id);
            let cancel_token = CancellationToken::new();
            let Some(handoff) = self.session_handoff_snapshot(&session_id) else {
                return Err(ActiveTurnStartError::MissingHandoff);
            };
            let plan_mode_instructions = self.session_plan_mode_instructions(&session_id);
            let active_turn = build_handles(ActiveTurnStartState {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                cancel_token,
                handoff,
                plan_mode_instructions,
            });
            Ok((turn_id, active_turn))
        });
        if result.is_ok() {
            self.session_activity.touch(activity_session_id);
        }
        result
    }

    /// Test-only blind install. Production turn start goes through
    /// [`Self::start_active_turn_for_session`], which check-and-installs in one
    /// critical section.
    #[cfg(test)]
    pub(super) fn install_active_turn(
        &self,
        session_id: coco_types::SessionId,
        active_turn: ActiveTurnHandles,
    ) {
        self.turn_state.install_active_turn(session_id, active_turn);
    }

    pub(super) fn take_active_turn(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Option<ActiveTurnHandles> {
        self.turn_state.take_active_turn(session_id)
    }

    pub(super) fn clear_active_turn(&self, session_id: &coco_types::SessionId) {
        if self.turn_state.clear_active_turn(session_id)
            && self.session_handoff_snapshot(session_id).is_some()
        {
            self.session_activity.touch(session_id.clone());
        }
    }

    pub(super) async fn clear_scoped_session_state(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Option<ActiveTurnHandles> {
        let active_turn = self.take_active_turn(session_id);
        self.clear_turn_counter(session_id);
        self.clear_session_accounting(session_id);
        self.clear_session_handoff(session_id);
        self.clear_session_metadata(session_id);
        self.clear_session_plan_mode_instructions(session_id);
        active_turn
    }

    pub(super) async fn install_replacement_session_state(
        &self,
        replacement: ReplacementSessionState,
    ) -> bool {
        let prior_session = self.sole_session_handoff_id();
        let prior_was_active = prior_session.is_some();
        if let Some(prior) = prior_session.as_ref()
            && let Some(token) = self.active_turn_cancel_token(prior)
        {
            warn!(
                prior_session = %prior,
                new_session = %replacement.session_id,
                reason = replacement.cancel_reason,
                "SdkServer: replacing active session; cancelling in-flight turn"
            );
            token.cancel();
        }
        if let Some(prior) = prior_session.as_ref() {
            self.clear_active_turn(prior);
            self.clear_session_handoff(prior);
            if replacement.prior_cleanup == PriorSessionCleanup::Full {
                self.clear_turn_counter(prior);
                self.clear_session_accounting(prior);
                self.clear_session_metadata(prior);
                self.clear_session_plan_mode_instructions(prior);
            }
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
        prior_was_active
    }

    pub(super) fn install_scoped_replacement_session_state(
        &self,
        replacement: ReplacementSessionState,
    ) {
        if let Some(token) = self.active_turn_cancel_token(&replacement.session_id) {
            warn!(
                session_id = %replacement.session_id,
                reason = replacement.cancel_reason,
                "SdkServer: replacing scoped session state; cancelling in-flight turn"
            );
            token.cancel();
        }
        self.clear_active_turn(&replacement.session_id);
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

    #[cfg(test)]
    pub(super) fn sole_session_handoff_snapshot(
        &self,
    ) -> Option<(coco_types::SessionId, SessionHandoffState)> {
        self.scoped_sessions.sole_handoff_snapshot()
    }

    fn has_session_handoffs(&self) -> bool {
        self.scoped_sessions.has_handoffs()
    }

    fn sole_session_handoff_id(&self) -> Option<coco_types::SessionId> {
        self.scoped_sessions.sole_handoff_id()
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

    pub(super) fn enable_agent_progress_summaries(&self) {
        self.bootstrap_state.enable_agent_progress_summaries();
    }

    pub(super) fn agent_progress_summaries_enabled(&self) -> bool {
        self.bootstrap_state.agent_progress_summaries_enabled()
    }

    pub fn set_bypass_permissions_available(&self, available: bool) {
        self.bootstrap_state
            .set_bypass_permissions_available(available);
    }

    pub fn bypass_permissions_available(&self) -> bool {
        self.bootstrap_state.bypass_permissions_available()
    }

    pub(super) async fn workspace_cwd(&self) -> Result<PathBuf, HandlerResult> {
        let runtime = self.session_runtime_snapshot().await;
        if let Some(runtime) = runtime {
            return Ok(runtime.current_cwd().read().await.clone());
        }
        if let Some(session_id) = self.sole_session_handoff_id().as_ref()
            && let Some(metadata) = self.session_metadata_snapshot(session_id)
        {
            return Ok(PathBuf::from(metadata.cwd));
        }
        if let Some(cwd) = self.bootstrap_state.bootstrap_or_startup_cwd().await {
            return Ok(cwd);
        }
        Err(HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "workspace cwd is unavailable before session/start; provide session/start.cwd or install startup cwd".to_string(),
            data: None,
        })
    }

    pub(super) async fn set_pending_plan_mode_instructions(&self, instructions: Option<String>) {
        self.initialize_state
            .set_plan_mode_instructions(instructions)
            .await;
    }

    pub(super) async fn pending_plan_mode_instructions(&self) -> Option<String> {
        self.initialize_state.plan_mode_instructions().await
    }

    pub(super) async fn set_sdk_initialize_hooks(
        &self,
        hooks: Option<HashMap<coco_types::HookEventType, Vec<coco_types::HookCallbackMatcher>>>,
    ) {
        self.initialize_state.set_hooks(hooks).await;
    }

    pub(super) async fn sdk_initialize_hooks(
        &self,
    ) -> Option<HashMap<coco_types::HookEventType, Vec<coco_types::HookCallbackMatcher>>> {
        self.initialize_state.hooks().await
    }

    pub(super) async fn set_pending_sdk_agents(&self, agents: Vec<coco_types::AgentDefinition>) {
        self.initialize_state.set_sdk_agents(agents).await;
    }

    pub(super) async fn pending_sdk_agents(&self) -> Vec<coco_types::AgentDefinition> {
        self.initialize_state.sdk_agents().await
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

    pub fn install_file_history_for_startup(
        &self,
        history: SharedFileHistory,
        config_home: std::path::PathBuf,
    ) {
        self.file_history.install_for_startup(history, config_home);
    }

    pub async fn install_file_history(
        &self,
        history: Option<SharedFileHistory>,
        config_home: Option<std::path::PathBuf>,
    ) {
        self.file_history.install(history, config_home).await;
    }

    pub(super) async fn file_history_snapshot(&self) -> Option<SharedFileHistory> {
        self.file_history.history_snapshot().await
    }

    pub(super) async fn file_history_config_home_snapshot(&self) -> Option<std::path::PathBuf> {
        self.file_history.config_home_snapshot().await
    }

    /// Persist the last MCP-registration report for `server` (v4.2). Read by
    /// `handle_mcp_status` to surface the registered `tool_count` + skipped /
    /// tombstoned tools. Overwritten on every (re)connect.
    pub async fn record_mcp_registration_report(
        &self,
        server: &str,
        report: coco_tools::RegisterMcpToolsReport,
    ) {
        self.mcp_registration.record(server, report).await;
    }

    /// Drop the stored report for `server` on disconnect, so `mcp/status`
    /// falls back to the advertised count + empty skipped/tombstoned lists.
    pub async fn clear_mcp_registration_report(&self, server: &str) {
        self.mcp_registration.clear(server).await;
    }

    async fn mcp_registration_status_projection(
        &self,
        server: &str,
        advertised_tool_count: i32,
    ) -> McpRegistrationStatusProjection {
        self.mcp_registration
            .status_projection(server, advertised_tool_count)
            .await
    }

    /// Register an expected `approval/resolve`. Returns the receiver the
    /// agent-side code should `await` to get the client's decision.
    /// Callers are responsible for sending the matching `AskForApproval`
    /// ServerRequest to the client.
    pub async fn register_approval(
        &self,
        request_id: String,
    ) -> oneshot::Receiver<ApprovalResolveParams> {
        self.pending_client_requests
            .register_approval(request_id)
            .await
    }

    /// Register an expected `input/resolveUserInput`.
    pub async fn register_user_input(
        &self,
        request_id: String,
    ) -> oneshot::Receiver<UserInputResolveParams> {
        self.pending_client_requests
            .register_user_input(request_id)
            .await
    }

    /// Register an expected `elicitation/resolve`. Used when an MCP server
    /// sends an elicitation request to the agent, which then forwards it
    /// to the SDK client.
    pub async fn register_elicitation(
        &self,
        request_id: String,
    ) -> oneshot::Receiver<ElicitationResolveParams> {
        self.pending_client_requests
            .register_elicitation(request_id)
            .await
    }

    pub(super) async fn resolve_pending_approval(
        &self,
        request_id: &str,
        params: ApprovalResolveParams,
    ) -> ResolveOutcome {
        self.pending_client_requests
            .resolve_approval(request_id, params)
            .await
    }

    pub(super) async fn resolve_pending_user_input(
        &self,
        request_id: &str,
        params: UserInputResolveParams,
    ) -> ResolveOutcome {
        self.pending_client_requests
            .resolve_user_input(request_id, params)
            .await
    }

    pub(super) async fn resolve_pending_elicitation(
        &self,
        request_id: &str,
        params: ElicitationResolveParams,
    ) -> ResolveOutcome {
        self.pending_client_requests
            .resolve_elicitation(request_id, params)
            .await
    }

    pub(super) async fn cancel_pending_client_request(
        &self,
        request_id: &str,
    ) -> Option<&'static str> {
        self.pending_client_requests.cancel(request_id).await
    }

    /// Issue an outbound ServerRequest on the provided transport and
    /// await the matching response.
    /// Generates a fresh monotonically-decreasing `RequestId` (starting
    /// at -1), registers an oneshot in `pending_server_requests`, enqueues
    /// a `JsonRpcFrame::Request` through the ordered writer, and awaits the
    /// receiver. The bridge reader wakes the receiver when the client replies
    /// with a matching `Success`/`Error` frame.
    /// Returns:
    /// - `Ok (JsonRpcFrame::Success (r))` — client replied successfully
    /// - `Ok (JsonRpcFrame::Error (e))` — client replied with an error
    /// - `Err(...)` — transport send failed or the oneshot was dropped
    /// (e.g. the transport closed before the client replied)
    pub async fn send_server_request(
        &self,
        transport: &Arc<dyn SdkTransport>,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<JsonRpcFrame> {
        let (request_id, rx, mut pending_guard) = self.server_requests.register_request().await;
        let request_id_for_error = request_id.as_display();

        // Write the request through the dispatcher's ordered outbound
        // queue. The fallback to `transport.send` that used to live
        // here was removed — it bypassed the single-writer ordering
        // guarantee for any call made before
        // `SdkServer::run_app_server_connection()`. Callers must wait for the
        // AppServer bridge to have populated `outbound_tx`; tests that need
        // this wait explicitly.
        let frame = JsonRpcFrame::Request(TransportJsonRpcRequest::new(
            crate::sdk_server::transport::json_rpc_id_from_request_id(request_id.clone()),
            method,
            Some(params),
        ));
        // CRITICAL: scope the Sender clone tightly so it drops BEFORE
        // we `rx.await`. Holding the Sender across the await keeps
        // an extra reference on the outbound channel and **prevents
        // writer-task shutdown** when the dispatcher's main loop
        // exits — the writer would wait for all Senders to drop,
        // but this spawned task holds one open while suspended on
        // `rx.await`. Result: `server_task.await` hangs forever on
        // clean shutdown. See test
        // `sdk_mcp_status_reports_pending_while_connect_waits_for_route_response`.
        let _ = transport;
        {
            let outbound_tx = self.connection_state.outbound_tx_snapshot().await;
            let Some(tx) = outbound_tx else {
                anyhow::bail!(
                    "send_server_request: outbound queue not initialized (server not yet running)"
                );
            };
            if tx.send(OutboundMessage::JsonRpcFrame(frame)).await.is_err() {
                anyhow::bail!("failed to send server request: outbound queue closed");
            }
        } // `tx` dropped here; writer task can shut down independently.

        // Await the client's reply. If the sender is dropped
        // (e.g. transport closed), RecvError propagates.
        match rx.await {
            Ok(reply) => {
                // `resolve_server_request_frame` already removed the entry
                // from the map when it delivered the reply. Tell the
                // guard to skip its cleanup on drop.
                pending_guard.disarm();
                Ok(reply)
            }
            Err(_) => {
                // Sender dropped without a reply — treat as cancelled.
                // Guard will clean up.
                anyhow::bail!("server request {request_id_for_error} cancelled: no reply received")
            }
        }
    }

    /// Deliver an inbound AppServer `Success`/`Error` frame to the matching
    /// pending SDK hook/MCP server request, if any.
    ///
    /// Unmatched frames continue through the AppServer adapter unchanged.
    pub async fn resolve_server_request_frame(&self, frame: JsonRpcFrame) -> bool {
        self.server_requests.resolve_frame(frame).await
    }

    #[cfg(test)]
    pub(super) async fn pending_server_request_count(&self) -> usize {
        self.server_requests.len().await
    }

    #[cfg(test)]
    pub(super) async fn pending_server_requests_is_empty(&self) -> bool {
        self.server_requests.is_empty().await
    }
}

impl std::fmt::Debug for SdkServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SdkServerState")
            .field("session", &"RwLock<..>")
            .field("turn_runner", &"TurnRunnerState")
            .field("session_runtime", &"SessionRuntimeState")
            .field("turn_state", &"TurnState")
            .field("scoped_sessions", &"ScopedSessionState")
            .field("initialize_state", &"InitializeState")
            .field("pending_client_requests", &"PendingClientRequestState")
            .field("server_requests", &"ServerRequestState")
            .field("mcp_registration", &"McpRegistrationState")
            .field("bootstrap_state", &"BootstrapState")
            .field("runtime_reload", &"RuntimeReloadState")
            .field("runtime_replacement", &"RuntimeReplacementState")
            .field(
                "next_server_request_id",
                &self.server_requests.next_id_for_debug(),
            )
            .field("connection_state", &"ConnectionState")
            .field("session_store", &"SessionStore")
            .field("file_history", &"FileHistoryStateSlot")
            .field("mcp_manager", &"McpManagerState")
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

/// State-owned handles for one active SDK turn.
pub(super) struct ActiveTurnHandles {
    pub cancel_token: CancellationToken,
    pub turn_task: tokio::task::JoinHandle<()>,
    pub forwarder_task: tokio::task::JoinHandle<()>,
}

pub(super) struct ArchivedSessionState {
    pub result: coco_types::SessionResultParams,
    pub active_turn: Option<ActiveTurnHandles>,
}

pub(super) enum ArchiveSessionError {
    NoActiveSession,
    SessionMismatch {
        active: coco_types::SessionId,
        requested: coco_types::SessionId,
    },
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
    pub cancel_reason: &'static str,
}

pub(super) struct StartedSessionState {
    pub session_id: coco_types::SessionId,
    pub metadata: SessionMetadata,
    pub handoff: SessionHandoffState,
    pub plan_mode_instructions: Option<String>,
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
