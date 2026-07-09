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
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use coco_types::ApprovalResolveParams;
use coco_types::ClientRequest;
use coco_types::CoreEvent;
use coco_types::ElicitationResolveParams;
use coco_types::JsonRpcMessage;
use coco_types::JsonRpcRequest;
use coco_types::RequestId;
use coco_types::UserInputResolveParams;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::warn;

use crate::sdk_server::outbound::OutboundMessage;
use crate::sdk_server::pending_map::PendingMap;
use crate::sdk_server::transport::SdkTransport;

pub mod config;
pub mod mcp;
pub mod rewind;
pub mod runtime;
pub mod session;
pub mod turn;

/// The SDK protocol version coco-rs speaks.
pub const PROTOCOL_VERSION: &str = "1.0";

/// Default model id reported by `initialize` and used when `session/start` /
/// `setModel` omit a model param.
pub const DEFAULT_SDK_MODEL: &str = "claude-opus-4-6";

/// Default fast-mode / secondary model id advertised by `initialize`.
pub const DEFAULT_SDK_FAST_MODEL: &str = "claude-sonnet-4-6";

/// RAII cleanup for a pending `send_server_request` entry.
/// The `send_server_request` function registers a oneshot sender in
/// `SdkServerState.pending_server_requests` before writing the request
/// to the transport. On the happy path, `resolve_server_request` removes
/// the entry when the reply arrives. On the cancelled path (e.g. caller
/// wraps the await in `tokio::select!` with a cancel token and the cancel
/// branch fires), the future is dropped mid-await — without this guard,
/// the entry would leak in the HashMap until state drop.
/// The guard holds a reference to the pending map and uses synchronous
/// `try_lock` in its `Drop` impl. If the mutex is contended at drop time
/// (another task is mid-write), the entry leaks — but that's a very
/// narrow window and the leak is bounded by concurrency.
struct PendingRequestGuard<'a> {
    map: &'a Mutex<HashMap<RequestId, oneshot::Sender<JsonRpcMessage>>>,
    request_id: RequestId,
    /// Set to `false` after `resolve_server_request` has already
    /// removed the entry (i.e. the happy-path Ok(reply) return).
    active: bool,
}

impl Drop for PendingRequestGuard<'_> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Ok(mut map) = self.map.try_lock() {
            map.remove(&self.request_id);
        }
        // If try_lock fails, accept the leak. It's bounded and will be
        // reclaimed when SdkServerState is dropped.
    }
}

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
    /// O(history) deep clone per turn.
    /// - `event_tx`: the channel on which CoreEvents must be emitted.
    /// The dispatcher's notification forwarder reads from this channel
    /// and writes JsonRpc notifications to the transport.
    /// - `cancel`: cancellation token. `turn/interrupt` triggers this.
    /// Returning `Ok(())` signals a clean turn completion. Returning an
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
/// impl lives in `coco-cli` where every source is already imported; tests
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
pub struct SdkServerState {
    /// The runner that executes turns. Defaulted to `NotImplementedRunner`.
    /// Stored behind `RwLock` so `SdkServer::set_turn_runner()` can
    /// install a real runner after the state is already shared (used
    /// by the approval-bridge wiring path where the bridge needs a
    /// reference to the live state before the runner is constructed).
    pub turn_runner: RwLock<Arc<dyn TurnRunner>>,
    /// Per-session counters used to mint SDK turn ids.
    ///
    /// Kept outside session handoff state so turn identity allocation is keyed
    /// independently.
    pub turn_counters: StdMutex<HashMap<coco_types::SessionId, i32>>,
    /// Per-session aggregate accounting for SDK archive/result summaries.
    ///
    /// Kept outside session handoff state so once-per-session result
    /// aggregation is keyed independently.
    pub session_accounting: StdMutex<HashMap<coco_types::SessionId, SessionAccounting>>,
    /// Active turn handles keyed by session id.
    ///
    /// Kept outside session handoff state so cancellation and task-drain handles
    /// are keyed independently.
    active_turns: StdMutex<HashMap<coco_types::SessionId, ActiveTurnHandles>>,
    /// Per-session history and live app state handed to SDK turns.
    ///
    /// Stored outside runtime handles so SDK/AppServer turns can share the same
    /// keyed handoff state.
    session_handoffs: StdMutex<HashMap<coco_types::SessionId, SessionHandoffState>>,
    /// Legacy SDK session metadata keyed by session id.
    ///
    /// Kept as keyed metadata outside runtime handles for legacy and
    /// AppServer-scoped requests.
    session_metadata: StdMutex<HashMap<coco_types::SessionId, SessionMetadata>>,
    /// Per-session SDK plan-mode workflow override from `initialize`.
    ///
    /// Kept as keyed metadata outside runtime handles for initialize-scoped
    /// handoff strings.
    session_plan_mode_instructions: StdMutex<HashMap<coco_types::SessionId, String>>,
    /// Pending `approval/askForApproval` ServerRequests awaiting a client
    /// `approval/resolve`. Keyed by `request_id`.
    pub pending_approvals: PendingMap<ApprovalResolveParams>,
    /// Pending `input/requestUserInput` ServerRequests awaiting a client
    /// `input/resolveUserInput`.
    pub pending_user_input: PendingMap<UserInputResolveParams>,
    /// Pending elicitation ServerRequests awaiting a client
    /// `elicitation/resolve`.
    pub pending_elicitations: PendingMap<ElicitationResolveParams>,
    /// Pending ServerRequests (server→client) awaiting a
    /// `JsonRpcMessage::Response` or `JsonRpcMessage::Error` reply.
    /// Keyed by the server-issued `RequestId`.
    /// Populated by [`SdkServerState::send_server_request`] when an
    /// outbound request is written to the transport; drained by the
    /// AppServer bridge reader when the matching response arrives.
    pub pending_server_requests: Mutex<HashMap<RequestId, oneshot::Sender<JsonRpcMessage>>>,
    /// Monotonic counter for issuing unique request IDs for outbound
    /// ServerRequests. Uses negative integers to avoid colliding with
    /// client-issued IDs (which are typically non-negative).
    pub next_server_request_id: AtomicI64,
    /// Transport handle shared with the AppServer bridge. Populated before
    /// startup and refreshed by `SdkServer::run_app_server_connection`; used by
    /// the approval bridge and other ServerRequest-emitting code paths. `None`
    /// in tests that construct `SdkServerState` directly.
    pub transport: RwLock<Option<Arc<dyn SdkTransport>>>,
    /// Ordered outbound queue owned by the running dispatcher. When set,
    /// server requests use this queue instead of writing directly to the
    /// transport so requests, replies, and notifications share one writer.
    pub outbound_tx: RwLock<Option<mpsc::Sender<OutboundMessage>>>,
    /// Optional disk-backed [`coco_session::SessionManager`] used by
    /// the `session/list`, `session/read`, `session/resume` handlers
    /// to browse and resume historical sessions. When `None`, those
    /// handlers reply with `METHOD_NOT_FOUND` (session persistence is
    /// disabled). The CLI entry point (`run_sdk_mode`) wires one
    /// pointing at `config home/sessions`; in-memory tests that don't
    /// exercise session/list can leave it as `None`.
    pub session_manager: RwLock<Option<Arc<coco_session::SessionManager>>>,
    /// Optional shared file-history state used by `control/rewindFiles`.
    /// When `None`, that handler errors with `INVALID_REQUEST`
    /// ("file history not enabled"). The CLI entry point wires a
    /// fresh `FileHistoryState` at startup; tests that don't exercise
    /// rewind can leave it as `None`.
    pub file_history: RwLock<Option<Arc<RwLock<coco_context::FileHistoryState>>>>,
    /// Config home directory used for file-history backups (resolved
    /// from `coco_config::global_config::config_home()` at CLI startup).
    /// Used in conjunction with `file_history` above.
    pub file_history_config_home: RwLock<Option<std::path::PathBuf>>,
    /// Optional MCP connection manager used by the `mcp/setServers`,
    /// `mcp/reconnect`, `mcp/toggle` handlers. The manager is wrapped
    /// in `tokio::sync::Mutex` (not `RwLock`) because `register_server`
    /// requires `&mut self` while `connect`/`disconnect` only need
    /// `&self`. The Mutex serializes both kinds of access — fine for
    /// these infrequent runtime-control operations.
    /// When `None`, the MCP lifecycle handlers respond with
    /// `INVALID_REQUEST` ("MCP manager not enabled").
    pub mcp_manager: RwLock<Option<Arc<Mutex<coco_mcp::McpConnectionManager>>>>,
    /// Optional [`InitializeBootstrap`] provider used by `handle_initialize`
    /// to populate `commands`, `agents`, `account`, `output_style`, etc.
    /// When `None`, the handler returns empty / default values for those
    /// fields so the wire format stays TS-conformant.
    pub initialize_bootstrap: RwLock<Option<Arc<dyn InitializeBootstrap>>>,
    /// Startup cwd captured by the CLI entrypoint before requests arrive.
    ///
    /// Used by session/config requests before a session or runtime exists.
    pub startup_cwd: RwLock<Option<PathBuf>>,
    /// Whether the SDK client opted into per-spawn periodic
    /// AgentSummary timers via `initialize { agentProgressSummaries: true }`.
    ///  `getSdkAgentProgressSummariesEnabled`.
    /// Default `false`. session/start copies this onto the new
    /// session's `ToolAppState.agent_progress_summaries_enabled`.
    pub agent_progress_summaries_enabled: std::sync::atomic::AtomicBool,
    /// Whether the process was authorized to transition into
    /// `BypassPermissions` at CLI startup (either via
    /// `--dangerously-skip-permissions` or
    /// `--allow-dangerously-skip-permissions`, subject to the policy
    /// killswitch). Consulted by `handle_set_permission_mode` to
    /// reject SDK-originated bypass requests mid-session when the
    /// flag was not passed.
    ///  — mid-session SDK switches
    /// to `bypassPermissions` are rejected with an explicit error
    /// when `isBypassPermissionsModeAvailable` is false.
    pub bypass_permissions_available: std::sync::atomic::AtomicBool,
    /// Process-shared `SessionHandle`. Set by `run_sdk_mode` at startup and
    /// swapped by AppServer-backed SDK `session/start` / `session/resume`
    /// replacement paths. Legacy handlers still read it for tests and
    /// non-replacement fixtures. `None` only in tests that don't wire a
    /// runtime.
    pub session_runtime: RwLock<Option<crate::session_runtime::SessionHandle>>,
    /// Runtime-owned SDK reload subscriber for the currently installed
    /// `session_runtime`. Replaced whenever SDK AppServer start/resume swaps
    /// in a fresh runtime.
    pub sdk_runtime_reload_subscription: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Optional SDK production runtime replacement context. When installed,
    /// AppServer bridge `session/start` / `session/resume` builds a fresh
    /// runtime through this factory and swaps `session_runtime` instead of
    /// reusing the previously installed fused runtime.
    pub runtime_replacement: RwLock<Option<RuntimeReplacementContext>>,

    /// Agent definitions pushed via `initialize.agents`
    /// (`z.record(z.string(), AgentDefinitionSchema)`). Stashed here at `initialize` time and
    /// drained when the per-session `AgentDefinitionStore` is built so
    /// SDK-supplied agents land in the catalog as `AgentSource::FlagSettings` entries.
    /// `RwLock` (not `Mutex`) because `agents()` in `CliInitializeBootstrap`
    /// reads concurrently with other initialize-time accessors.
    pub pending_sdk_agents: RwLock<Vec<coco_types::AgentDefinition>>,

    /// Last `initialize.planModeInstructions` value. `session/start` snapshots
    /// it onto the new session so later initialize calls do not mutate an
    /// already-active session.
    pub pending_plan_mode_instructions: RwLock<Option<String>>,

    /// Last SDK `initialize.hooks` registration. Runtime-backed resume builds
    /// a fresh `SessionRuntime`, so bridge-level replacement replays these
    /// session-scoped SDK callback hooks onto the new runtime.
    pub sdk_initialize_hooks:
        RwLock<Option<HashMap<coco_types::HookEventType, Vec<coco_types::HookCallbackMatcher>>>>,

    /// Last `RegisterMcpToolsReport` per MCP server (v4.2). Written by the
    /// register call sites; read by `handle_mcp_status` to source the
    /// registered `tool_count` + skipped / tombstoned tools. Cleared on
    /// disconnect; bounded by the number of distinct server names seen.
    pub mcp_registration_reports: RwLock<HashMap<String, coco_tools::RegisterMcpToolsReport>>,
}

impl Default for SdkServerState {
    fn default() -> Self {
        Self {
            turn_runner: RwLock::new(Arc::new(NotImplementedRunner) as Arc<dyn TurnRunner>),
            turn_counters: StdMutex::new(HashMap::new()),
            session_accounting: StdMutex::new(HashMap::new()),
            active_turns: StdMutex::new(HashMap::new()),
            session_handoffs: StdMutex::new(HashMap::new()),
            session_metadata: StdMutex::new(HashMap::new()),
            session_plan_mode_instructions: StdMutex::new(HashMap::new()),
            pending_approvals: PendingMap::new(),
            pending_user_input: PendingMap::new(),
            pending_elicitations: PendingMap::new(),
            pending_server_requests: Mutex::new(HashMap::new()),
            // Start at -1 and decrement. Keeps us out of the typical
            // client-issued integer range and makes outbound IDs
            // visually distinctive in logs.
            next_server_request_id: AtomicI64::new(-1),
            transport: RwLock::new(None),
            outbound_tx: RwLock::new(None),
            session_manager: RwLock::new(None),
            file_history: RwLock::new(None),
            file_history_config_home: RwLock::new(None),
            mcp_manager: RwLock::new(None),
            initialize_bootstrap: RwLock::new(None),
            startup_cwd: RwLock::new(None),
            agent_progress_summaries_enabled: std::sync::atomic::AtomicBool::new(false),
            bypass_permissions_available: std::sync::atomic::AtomicBool::new(false),
            session_runtime: RwLock::new(None),
            sdk_runtime_reload_subscription: Mutex::new(None),
            runtime_replacement: RwLock::new(None),
            pending_sdk_agents: RwLock::new(Vec::new()),
            pending_plan_mode_instructions: RwLock::new(None),
            sdk_initialize_hooks: RwLock::new(None),
            mcp_registration_reports: RwLock::new(HashMap::new()),
        }
    }
}

impl SdkServerState {
    /// Best-effort current session id for process-level bridge fallbacks that
    /// do not have an AppServer request context.
    pub async fn runtime_or_active_session_id(&self) -> Option<coco_types::SessionId> {
        let runtime = self.session_runtime.read().await.clone();
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
        let mut counters = match self.turn_counters.lock() {
            Ok(counters) => counters,
            Err(poisoned) => poisoned.into_inner(),
        };
        let counter = counters.entry(session_id.clone()).or_insert(0);
        *counter = counter.saturating_add(1);
        coco_types::TurnId::from(format!("turn-{session_id}-{counter}"))
    }

    pub(super) fn clear_turn_counter(&self, session_id: &coco_types::SessionId) {
        let mut counters = match self.turn_counters.lock() {
            Ok(counters) => counters,
            Err(poisoned) => poisoned.into_inner(),
        };
        counters.remove(session_id);
    }

    pub(super) fn reset_session_accounting(&self, session_id: coco_types::SessionId) {
        let mut accounting = match self.session_accounting.lock() {
            Ok(accounting) => accounting,
            Err(poisoned) => poisoned.into_inner(),
        };
        accounting.insert(session_id, SessionAccounting::new());
    }

    pub(super) fn clear_session_accounting(&self, session_id: &coco_types::SessionId) {
        let mut accounting = match self.session_accounting.lock() {
            Ok(accounting) => accounting,
            Err(poisoned) => poisoned.into_inner(),
        };
        accounting.remove(session_id);
    }

    pub(super) fn session_accounting_snapshot(
        &self,
        session_id: &coco_types::SessionId,
    ) -> SessionAccounting {
        let accounting = match self.session_accounting.lock() {
            Ok(accounting) => accounting,
            Err(poisoned) => poisoned.into_inner(),
        };
        accounting
            .get(session_id)
            .cloned()
            .unwrap_or_else(SessionAccounting::new)
    }

    pub(super) fn accumulate_session_result(
        &self,
        session_id: &coco_types::SessionId,
        params: &coco_types::SessionResultParams,
    ) {
        let mut accounting = match self.session_accounting.lock() {
            Ok(accounting) => accounting,
            Err(poisoned) => poisoned.into_inner(),
        };
        let entry = accounting
            .entry(session_id.clone())
            .or_insert_with(SessionAccounting::new);
        entry.stats.accumulate(params);
    }

    pub(super) fn has_active_turn(&self, session_id: &coco_types::SessionId) -> bool {
        let active_turns = match self.active_turns.lock() {
            Ok(active_turns) => active_turns,
            Err(poisoned) => poisoned.into_inner(),
        };
        active_turns.contains_key(session_id)
    }

    pub(super) fn active_turn_cancel_token(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Option<CancellationToken> {
        let active_turns = match self.active_turns.lock() {
            Ok(active_turns) => active_turns,
            Err(poisoned) => poisoned.into_inner(),
        };
        active_turns
            .get(session_id)
            .map(|turn| turn.cancel_token.clone())
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
        Ok(ShortcutTurnState {
            session_id,
            turn_id,
            history: handoff.history,
        })
    }

    pub(super) fn start_active_turn_for_session<F>(
        &self,
        session_id: coco_types::SessionId,
        build_handles: F,
    ) -> Result<coco_types::TurnId, ActiveTurnStartError>
    where
        F: FnOnce(ActiveTurnStartState) -> ActiveTurnHandles,
    {
        if self.has_active_turn(&session_id) {
            return Err(ActiveTurnStartError::TurnAlreadyRunning);
        }
        let turn_id = self.next_turn_id(&session_id);
        let cancel_token = CancellationToken::new();
        let Some(handoff) = self.session_handoff_snapshot(&session_id) else {
            return Err(ActiveTurnStartError::MissingHandoff);
        };
        let plan_mode_instructions = self.session_plan_mode_instructions(&session_id);
        let active_session_id = session_id.clone();
        let active_turn = build_handles(ActiveTurnStartState {
            session_id,
            turn_id: turn_id.clone(),
            cancel_token,
            handoff,
            plan_mode_instructions,
        });
        self.install_active_turn(active_session_id, active_turn);
        Ok(turn_id)
    }

    pub(super) fn install_active_turn(
        &self,
        session_id: coco_types::SessionId,
        active_turn: ActiveTurnHandles,
    ) {
        let mut active_turns = match self.active_turns.lock() {
            Ok(active_turns) => active_turns,
            Err(poisoned) => poisoned.into_inner(),
        };
        active_turns.insert(session_id, active_turn);
    }

    pub(super) fn take_active_turn(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Option<ActiveTurnHandles> {
        let mut active_turns = match self.active_turns.lock() {
            Ok(active_turns) => active_turns,
            Err(poisoned) => poisoned.into_inner(),
        };
        active_turns.remove(session_id)
    }

    pub(super) fn clear_active_turn(&self, session_id: &coco_types::SessionId) {
        let mut active_turns = match self.active_turns.lock() {
            Ok(active_turns) => active_turns,
            Err(poisoned) => poisoned.into_inner(),
        };
        active_turns.remove(session_id);
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
        let mut handoffs = match self.session_handoffs.lock() {
            Ok(handoffs) => handoffs,
            Err(poisoned) => poisoned.into_inner(),
        };
        handoffs.insert(session_id, handoff);
    }

    pub fn session_handoff_snapshot(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Option<SessionHandoffState> {
        let handoffs = match self.session_handoffs.lock() {
            Ok(handoffs) => handoffs,
            Err(poisoned) => poisoned.into_inner(),
        };
        handoffs.get(session_id).cloned()
    }

    pub(super) fn sole_session_handoff_snapshot(
        &self,
    ) -> Option<(coco_types::SessionId, SessionHandoffState)> {
        let handoffs = match self.session_handoffs.lock() {
            Ok(handoffs) => handoffs,
            Err(poisoned) => poisoned.into_inner(),
        };
        if handoffs.len() == 1 {
            handoffs
                .iter()
                .next()
                .map(|(session_id, handoff)| (session_id.clone(), handoff.clone()))
        } else {
            None
        }
    }

    fn has_session_handoffs(&self) -> bool {
        let handoffs = match self.session_handoffs.lock() {
            Ok(handoffs) => handoffs,
            Err(poisoned) => poisoned.into_inner(),
        };
        !handoffs.is_empty()
    }

    fn sole_session_handoff_id(&self) -> Option<coco_types::SessionId> {
        self.sole_session_handoff_snapshot()
            .map(|(session_id, _)| session_id)
    }

    pub(super) fn clear_session_handoff(&self, session_id: &coco_types::SessionId) {
        let mut handoffs = match self.session_handoffs.lock() {
            Ok(handoffs) => handoffs,
            Err(poisoned) => poisoned.into_inner(),
        };
        handoffs.remove(session_id);
    }

    pub(super) fn set_session_metadata(
        &self,
        session_id: coco_types::SessionId,
        metadata: SessionMetadata,
    ) {
        let mut all_metadata = match self.session_metadata.lock() {
            Ok(metadata) => metadata,
            Err(poisoned) => poisoned.into_inner(),
        };
        all_metadata.insert(session_id, metadata);
    }

    pub(super) fn session_metadata_snapshot(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Option<SessionMetadata> {
        let all_metadata = match self.session_metadata.lock() {
            Ok(metadata) => metadata,
            Err(poisoned) => poisoned.into_inner(),
        };
        all_metadata.get(session_id).cloned()
    }

    pub(super) fn update_session_model(
        &self,
        session_id: &coco_types::SessionId,
        model: String,
    ) -> Option<String> {
        let mut all_metadata = match self.session_metadata.lock() {
            Ok(metadata) => metadata,
            Err(poisoned) => poisoned.into_inner(),
        };
        let metadata = all_metadata.get_mut(session_id)?;
        Some(std::mem::replace(&mut metadata.model, model))
    }

    pub(super) fn clear_session_metadata(&self, session_id: &coco_types::SessionId) {
        let mut all_metadata = match self.session_metadata.lock() {
            Ok(metadata) => metadata,
            Err(poisoned) => poisoned.into_inner(),
        };
        all_metadata.remove(session_id);
    }

    pub(super) fn set_session_plan_mode_instructions(
        &self,
        session_id: coco_types::SessionId,
        instructions: Option<String>,
    ) {
        let mut all_instructions = match self.session_plan_mode_instructions.lock() {
            Ok(instructions) => instructions,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(instructions) = instructions {
            all_instructions.insert(session_id, instructions);
        } else {
            all_instructions.remove(&session_id);
        }
    }

    pub(super) fn session_plan_mode_instructions(
        &self,
        session_id: &coco_types::SessionId,
    ) -> Option<String> {
        let all_instructions = match self.session_plan_mode_instructions.lock() {
            Ok(instructions) => instructions,
            Err(poisoned) => poisoned.into_inner(),
        };
        all_instructions.get(session_id).cloned()
    }

    pub(super) fn clear_session_plan_mode_instructions(&self, session_id: &coco_types::SessionId) {
        let mut all_instructions = match self.session_plan_mode_instructions.lock() {
            Ok(instructions) => instructions,
            Err(poisoned) => poisoned.into_inner(),
        };
        all_instructions.remove(session_id);
    }

    pub(super) async fn workspace_cwd(&self) -> Result<PathBuf, HandlerResult> {
        let runtime = self.session_runtime.read().await.clone();
        if let Some(runtime) = runtime {
            return Ok(runtime.current_cwd().read().await.clone());
        }
        if let Some(session_id) = self.sole_session_handoff_id().as_ref()
            && let Some(metadata) = self.session_metadata_snapshot(session_id)
        {
            return Ok(PathBuf::from(metadata.cwd));
        }
        if let Some(bootstrap) = self.initialize_bootstrap.read().await.as_ref() {
            return Ok(bootstrap.cwd().await);
        }
        if let Some(cwd) = self.startup_cwd.read().await.as_ref() {
            return Ok(cwd.clone());
        }
        Err(HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "workspace cwd is unavailable before session/start; provide session/start.cwd or install startup cwd".to_string(),
            data: None,
        })
    }

    /// Persist the last MCP-registration report for `server` (v4.2). Read by
    /// `handle_mcp_status` to surface the registered `tool_count` + skipped /
    /// tombstoned tools. Overwritten on every (re)connect.
    pub async fn record_mcp_registration_report(
        &self,
        server: &str,
        report: coco_tools::RegisterMcpToolsReport,
    ) {
        self.mcp_registration_reports
            .write()
            .await
            .insert(server.to_string(), report);
    }

    /// Drop the stored report for `server` on disconnect, so `mcp/status`
    /// falls back to the advertised count + empty skipped/tombstoned lists.
    pub async fn clear_mcp_registration_report(&self, server: &str) {
        self.mcp_registration_reports.write().await.remove(server);
    }

    /// Register an expected `approval/resolve`. Returns the receiver the
    /// agent-side code should `await` to get the client's decision.
    /// Callers are responsible for sending the matching `AskForApproval`
    /// ServerRequest to the client.
    pub async fn register_approval(
        &self,
        request_id: String,
    ) -> oneshot::Receiver<ApprovalResolveParams> {
        self.pending_approvals.register(request_id).await
    }

    /// Register an expected `input/resolveUserInput`.
    pub async fn register_user_input(
        &self,
        request_id: String,
    ) -> oneshot::Receiver<UserInputResolveParams> {
        self.pending_user_input.register(request_id).await
    }

    /// Register an expected `elicitation/resolve`. Used when an MCP server
    /// sends an elicitation request to the agent, which then forwards it
    /// to the SDK client.
    pub async fn register_elicitation(
        &self,
        request_id: String,
    ) -> oneshot::Receiver<ElicitationResolveParams> {
        self.pending_elicitations.register(request_id).await
    }

    /// Issue an outbound ServerRequest on the provided transport and
    /// await the matching response.
    /// Generates a fresh monotonically-decreasing `RequestId` (starting
    /// at -1), registers an oneshot in `pending_server_requests`, writes
    /// the `JsonRpcRequest` onto the transport, and awaits the receiver.
    /// The dispatcher's inbound-message handler wakes the receiver when
    /// the client replies with a matching `Response`/`Error`.
    /// Returns:
    /// - `Ok(JsonRpcMessage::Response(r))` — client replied successfully
    /// - `Ok(JsonRpcMessage::Error(e))` — client replied with an error
    /// - `Err(...)` — transport send failed or the oneshot was dropped
    /// (e.g. the transport closed before the client replied)
    pub async fn send_server_request(
        &self,
        transport: &Arc<dyn SdkTransport>,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<JsonRpcMessage> {
        // Allocate a fresh id.
        let raw = self.next_server_request_id.fetch_sub(1, Ordering::SeqCst);
        let request_id = RequestId::Integer(raw);

        // Register a pending slot BEFORE sending so the response can't
        // race ahead of the insert.
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending_server_requests.lock().await;
            map.insert(request_id.clone(), tx);
        }

        // Drop-guard to clean up the pending slot if this future is
        // dropped before `rx.await` completes normally — e.g. the
        // caller wrapped us in a `tokio::select!` and the cancel branch
        // fired. Without this, cancelled approvals would leak an entry
        // in `pending_server_requests` until state drop.
        // The guard uses `try_lock` in its sync `Drop` impl. If the
        // mutex is contended at drop time, the entry leaks — but that's
        // a very narrow window (only contended while another caller is
        // reading/writing the map), and the leak is bounded.
        let mut pending_guard = PendingRequestGuard {
            map: &self.pending_server_requests,
            request_id: request_id.clone(),
            active: true,
        };

        // Write the request through the dispatcher's ordered outbound
        // queue. The fallback to `transport.send` that used to live
        // here was removed — it bypassed the single-writer ordering
        // guarantee for any call made before
        // `SdkServer::run_app_server_connection()`. Callers must wait for the
        // AppServer bridge to have populated `outbound_tx`; tests that need
        // this wait explicitly.
        let req = JsonRpcRequest {
            jsonrpc: coco_types::JSONRPC_VERSION.into(),
            request_id: request_id.clone(),
            method: method.into(),
            params,
        };
        let msg = JsonRpcMessage::Request(req);
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
            let outbound_tx = {
                let slot = self.outbound_tx.read().await;
                slot.clone()
            };
            let Some(tx) = outbound_tx else {
                anyhow::bail!(
                    "send_server_request: outbound queue not initialized (server not yet running)"
                );
            };
            if tx.send(OutboundMessage::JsonRpc(msg)).await.is_err() {
                anyhow::bail!("failed to send server request: outbound queue closed");
            }
        } // `tx` dropped here; writer task can shut down independently.

        // Await the client's reply. If the sender is dropped
        // (e.g. transport closed), RecvError propagates.
        match rx.await {
            Ok(reply) => {
                // `resolve_server_request` already removed the entry
                // from the map when it delivered the reply. Tell the
                // guard to skip its cleanup on drop.
                pending_guard.active = false;
                Ok(reply)
            }
            Err(_) => {
                // Sender dropped without a reply — treat as cancelled.
                // Guard will clean up.
                anyhow::bail!("server request {raw} cancelled: no reply received")
            }
        }
    }

    /// Deliver an inbound `Response`/`Error` to the pending server
    /// request with the matching JSON-RPC `id`, if any. Called by the
    /// dispatcher when it reads a message from the transport.
    /// Returns `true` if the message was routed to a pending request;
    /// `false` if no match was found (the client is replying to a
    /// request we don't have — usually a protocol confusion, logged
    /// but not fatal).
    pub async fn resolve_server_request(&self, msg: JsonRpcMessage) -> bool {
        let request_id = match &msg {
            JsonRpcMessage::Response(r) => r.request_id.clone(),
            JsonRpcMessage::Error(e) => e.request_id.clone(),
            _ => return false,
        };
        let mut map = self.pending_server_requests.lock().await;
        let Some(sender) = map.remove(&request_id) else {
            debug!(
                request_id = %request_id.as_display(),
                "resolve_server_request: no pending match"
            );
            return false;
        };
        // If the agent-side receiver has been dropped, the client's
        // reply is effectively lost. Log and move on.
        if sender.send(msg).is_err() {
            warn!(
                request_id = %request_id.as_display(),
                "resolve_server_request: receiver dropped before reply arrived"
            );
        }
        true
    }
}

impl std::fmt::Debug for SdkServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SdkServerState")
            .field("session", &"RwLock<..>")
            .field("turn_runner", &"RwLock<Arc<dyn TurnRunner>>")
            .field("turn_counters", &"Mutex<HashMap<SessionId, i32>>")
            .field(
                "session_accounting",
                &"Mutex<HashMap<SessionId, SessionAccounting>>",
            )
            .field(
                "active_turns",
                &"Mutex<HashMap<SessionId, ActiveTurnHandles>>",
            )
            .field(
                "session_handoffs",
                &"Mutex<HashMap<SessionId, SessionHandoffState>>",
            )
            .field(
                "session_metadata",
                &"Mutex<HashMap<SessionId, SessionMetadata>>",
            )
            .field(
                "session_plan_mode_instructions",
                &"Mutex<HashMap<SessionId, String>>",
            )
            .field("pending_approvals", &"PendingMap<..>")
            .field("pending_user_input", &"PendingMap<..>")
            .field("pending_elicitations", &"PendingMap<..>")
            .field("pending_server_requests", &"Mutex<HashMap<..>>")
            .field(
                "next_server_request_id",
                &self.next_server_request_id.load(Ordering::Relaxed),
            )
            .field("outbound_tx", &"RwLock<Option<Sender<..>>>")
            .field("session_manager", &"RwLock<Option<Arc<SessionManager>>>")
            .field(
                "file_history",
                &"RwLock<Option<Arc<RwLock<FileHistoryState>>>>",
            )
            .field("file_history_config_home", &"RwLock<Option<PathBuf>>")
            .field(
                "mcp_manager",
                &"RwLock<Option<Arc<Mutex<McpConnectionManager>>>>",
            )
            .field(
                "initialize_bootstrap",
                &"RwLock<Option<Arc<dyn InitializeBootstrap>>>",
            )
            .field("startup_cwd", &"RwLock<Option<PathBuf>>")
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
    pub runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    pub process_runtime: Arc<crate::process_runtime::ProcessRuntime>,
    pub cwd: PathBuf,
    pub requires_structured_output: bool,
}

/// Per-request context passed to handlers.
pub struct HandlerContext {
    /// Channel for forwarding CoreEvent notifications to the transport.
    /// Handlers that spawn a QueryEngine pass this as the engine's
    /// `event_tx`. Single-shot handlers (e.g., `initialize`) rarely use
    /// it; long-running handlers (e.g., `turn/start`) emit events here.
    pub notif_tx: mpsc::Sender<OutboundMessage>,

    /// Shared server state across requests.
    pub state: Arc<SdkServerState>,

    /// AppServer-derived session scope for the request connection.
    ///
    /// Set only when the connection has exactly one attached interactive
    /// surface. Handlers fall back to the installed runtime's scoped state,
    /// then to a sole keyed handoff when this is absent.
    pub scoped_session_id: Option<coco_types::SessionId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActiveSessionSource {
    Scoped,
    Runtime,
    ScopedState,
}

impl HandlerContext {
    pub fn has_scoped_session(&self) -> bool {
        self.scoped_session_id.is_some()
    }

    pub(super) async fn active_session_resolution(
        &self,
    ) -> Option<(coco_types::SessionId, ActiveSessionSource)> {
        if let Some(session_id) = &self.scoped_session_id {
            return Some((session_id.clone(), ActiveSessionSource::Scoped));
        }
        let runtime = self.state.session_runtime.read().await.clone();
        if let Some(runtime) = runtime {
            let session_id = runtime.current_typed_session_id().await;
            if self.state.session_handoff_snapshot(&session_id).is_some() {
                return Some((session_id, ActiveSessionSource::Runtime));
            }
        }
        if let Some(session_id) = self.state.sole_session_handoff_id() {
            return Some((session_id, ActiveSessionSource::ScopedState));
        }
        None
    }

    pub async fn active_session_id(&self) -> Option<coco_types::SessionId> {
        self.active_session_resolution()
            .await
            .map(|(session_id, _)| session_id)
    }

    pub(super) async fn workspace_cwd(&self) -> Result<PathBuf, HandlerResult> {
        if let Some(session_id) = &self.scoped_session_id
            && let Some(metadata) = self.state.session_metadata_snapshot(session_id)
        {
            return Ok(PathBuf::from(metadata.cwd));
        }
        self.state.workspace_cwd().await
    }
}

/// Result of dispatching a ClientRequest.
pub enum HandlerResult {
    /// Handler succeeded — carries the response `result` payload.
    Ok(Value),
    /// Handler failed with a JSON-RPC error.
    Err {
        code: i32,
        message: String,
        data: Option<Value>,
    },
    /// Handler is not implemented in the current phase. The dispatcher
    /// converts this to a `JsonRpcError` with `METHOD_NOT_FOUND`.
    NotImplemented(String),
}

impl HandlerResult {
    /// Shorthand for a successful empty response.
    pub fn ok_empty() -> Self {
        Self::Ok(Value::Null)
    }

    /// Build an Ok result from any serializable payload. Handler errors
    /// on serialization failure (rare in practice).
    pub fn ok<T: serde::Serialize>(value: T) -> Self {
        match serde_json::to_value(value) {
            Ok(v) => Self::Ok(v),
            Err(e) => Self::Err {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("result serialization failed: {e}"),
                data: None,
            },
        }
    }
}

/// Route a `ClientRequest` to its handler and return the result.
/// The dispatch is exhaustive — adding a new variant to `ClientRequest`
/// fails compilation here, enforcing that every method has a handler.
pub async fn dispatch_client_request(req: ClientRequest, ctx: HandlerContext) -> HandlerResult {
    match req {
        // === Session lifecycle ===
        ClientRequest::Initialize(params) => session::handle_initialize(params, &ctx).await,
        ClientRequest::SessionStart(params) => session::handle_session_start(*params, &ctx).await,
        ClientRequest::SessionResume(params) => session::handle_session_resume(params, &ctx).await,
        ClientRequest::SessionList => session::handle_session_list(&ctx).await,
        ClientRequest::SessionRead(params) => session::handle_session_read(params, &ctx).await,
        ClientRequest::SessionTurnsList(params) => {
            session::handle_session_turns_list(params, &ctx).await
        }
        ClientRequest::SessionSubscribe(_) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session/subscribe requires AppServer routing".to_string(),
            data: None,
        },
        ClientRequest::SessionArchive(params) => {
            session::handle_session_archive(params, &ctx).await
        }
        ClientRequest::SessionRename(params) => session::handle_session_rename(params, &ctx).await,
        ClientRequest::SessionToggleTag(params) => {
            session::handle_session_toggle_tag(params, &ctx).await
        }
        ClientRequest::SessionCost => runtime::handle_session_cost(&ctx).await,
        ClientRequest::SessionStatus => runtime::handle_session_status(&ctx).await,

        // === Turn control ===
        ClientRequest::TurnStart(params) => turn::handle_turn_start(params, &ctx).await,
        ClientRequest::TurnInterrupt => turn::handle_turn_interrupt(&ctx).await,

        // === Running task observability ===
        ClientRequest::TaskList => runtime::handle_task_list(&ctx).await,
        ClientRequest::TaskDetail(params) => runtime::handle_task_detail(params, &ctx).await,

        // === Approval + user input + elicitation ===
        ClientRequest::ApprovalResolve(params) => turn::handle_approval_resolve(params, &ctx).await,
        ClientRequest::UserInputResolve(params) => {
            turn::handle_user_input_resolve(params, &ctx).await
        }
        ClientRequest::ElicitationResolve(params) => {
            turn::handle_elicitation_resolve(params, &ctx).await
        }

        // === Runtime control ===
        ClientRequest::SetModel(params) => runtime::handle_set_model(params, &ctx).await,
        ClientRequest::SetModelRole(params) => runtime::handle_set_model_role(params, &ctx).await,
        ClientRequest::SetPermissionMode(params) => {
            runtime::handle_set_permission_mode(params, &ctx).await
        }
        ClientRequest::SetThinking(params) => runtime::handle_set_thinking(params, &ctx).await,
        ClientRequest::SetAgentColor(params) => runtime::handle_set_agent_color(params, &ctx).await,
        ClientRequest::ApplyPermissionUpdate(params) => {
            runtime::handle_apply_permission_update(params, &ctx).await
        }
        ClientRequest::ResetSessionPermissionRules => {
            runtime::handle_reset_session_permission_rules(&ctx).await
        }
        ClientRequest::StopTask(params) => runtime::handle_stop_task(params, &ctx).await,
        ClientRequest::RewindFiles(params) => rewind::handle_rewind_files(params, &ctx).await,
        ClientRequest::UpdateEnv(params) => runtime::handle_update_env(params, &ctx).await,
        ClientRequest::BackgroundAllTasks => runtime::handle_background_all_tasks(&ctx).await,

        // `keepAlive` is the simplest handler — respond with empty ok so
        // clients using it as a heartbeat get immediate acknowledgement.
        ClientRequest::KeepAlive => HandlerResult::ok_empty(),

        ClientRequest::CancelRequest(params) => turn::handle_cancel_request(params, &ctx).await,
        ClientRequest::AgentInterruptCurrentWork(params) => {
            runtime::handle_agent_interrupt_current_work(params, &ctx).await
        }

        // === Config ===
        ClientRequest::ConfigRead => config::handle_config_read(&ctx).await,
        ClientRequest::ConfigWrite(params) => config::handle_config_write(params, &ctx).await,

        // === TS P1 gap additions ===
        ClientRequest::McpStatus => mcp::handle_mcp_status(&ctx).await,
        ClientRequest::ContextUsage => runtime::handle_context_usage(&ctx).await,
        ClientRequest::McpSetServers(params) => mcp::handle_mcp_set_servers(params, &ctx).await,
        ClientRequest::McpReconnect(params) => mcp::handle_mcp_reconnect(params, &ctx).await,
        ClientRequest::McpToggle(params) => mcp::handle_mcp_toggle(params, &ctx).await,
        ClientRequest::PluginReload => runtime::handle_plugin_reload(&ctx).await,
        ClientRequest::HookReload => runtime::handle_hook_reload(&ctx).await,
        ClientRequest::ConfigApplyFlags(params) => {
            runtime::handle_config_apply_flags(params, &ctx).await
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
