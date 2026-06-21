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
//! - [`runtime`] — `setModel` / `setPermissionMode` / `setThinking` /
//!   `updateEnv` / `stopTask` / `context/usage` / `plugin/reload` /
//!   `config/applyFlags`
//! - [`config`] — `config/read` + `config/value/write`
//! - [`mcp`] — `mcp/status` / `mcp/setServers` / `mcp/reconnect` / `mcp/toggle`
//! - [`rewind`] — `control/rewindFiles`
//!
//! The dispatch match in [`dispatch_client_request`] is exhaustive — adding
//! a new `ClientRequest` variant fails compilation here, forcing a handler
//! to be written.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
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
///
/// The `send_server_request` function registers a oneshot sender in
/// `SdkServerState.pending_server_requests` before writing the request
/// to the transport. On the happy path, `resolve_server_request` removes
/// the entry when the reply arrives. On the cancelled path (e.g. caller
/// wraps the await in `tokio::select!` with a cancel token and the cancel
/// branch fires), the future is dropped mid-await — without this guard,
/// the entry would leak in the HashMap until state drop.
///
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
///
/// The sdk_server module doesn't depend on `coco-query`, so the dispatch
/// layer stays pure. In production, the CLI entry point wires a concrete
/// runner that spawns a `QueryEngine`. Tests inject mock runners that
/// emit scripted events.
pub trait TurnRunner: Send + Sync {
    /// Run a single turn.
    ///
    /// - `params`: the `turn/start` parameters from the client.
    /// - `handoff`: the narrow subset of `SessionHandle` the runner needs
    ///   (id, cwd, model, shared history). Stats and other per-session
    ///   state deliberately stay on the server-side slot to avoid an
    ///   O(history) deep clone per turn.
    /// - `event_tx`: the channel on which CoreEvents must be emitted.
    ///   The dispatcher's notification forwarder reads from this channel
    ///   and writes JsonRpc notifications to the transport.
    /// - `cancel`: cancellation token. `turn/interrupt` triggers this.
    ///
    /// Returning `Ok(())` signals a clean turn completion. Returning an
    /// error causes the server to emit a `turn/failed` notification (future)
    /// and log the error.
    fn run_turn<'a>(
        &'a self,
        params: coco_types::TurnStartParams,
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
        _handoff: TurnHandoff,
        _event_tx: mpsc::Sender<CoreEvent>,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async {
            anyhow::bail!(
                "SdkServer was constructed without a TurnRunner; \
                 call SdkServer::with_turn_runner() before run()"
            )
        })
    }
}

/// Narrow per-turn view of an active session handed to a [`TurnRunner`].
///
/// Holds only what the runner actually reads — session metadata used for
/// logging / `QueryEngineConfig`, plus the `Arc`-wrapped shared history so
/// the runner can thread messages across turns without taking ownership
/// of the whole `SessionHandle`. Crucially excludes `stats`,
/// `env_overrides`, `permission_denials`, and similar server-bookkeeping
/// state that was previously deep-cloned into the runner on every turn.
#[derive(Debug, Clone)]
pub struct TurnHandoff {
    pub session_id: String,
    pub cwd: String,
    pub model: String,
    pub history: Arc<Mutex<Vec<std::sync::Arc<coco_messages::Message>>>>,
    /// Session-scoped shared state. Attached to every turn's engine
    /// via `with_app_state` so plan-mode cadence + live permission
    /// mode propagate across turns AND mid-session mode toggles
    /// reach the engine. TS parity: `appState` is session-lifetime.
    pub app_state: Arc<RwLock<coco_types::ToolAppState>>,
    /// Session-scoped permission-mode override set by
    /// `control/setPermissionMode`. Used by `sdk_runner::run_turn`
    /// as a fallback when the `turn/start` params don't carry an
    /// explicit mode — before this wire-up the SessionHandle field
    /// was dead (no reader).
    pub permission_mode: Option<coco_types::PermissionMode>,
}

// ---------------------------------------------------------------------------
// InitializeBootstrap — cross-subsystem data provider for `initialize`
// ---------------------------------------------------------------------------

/// Provides the data fields that `InitializeResult` advertises to SDK clients.
///
/// `InitializeResult` is a cross-cutting bundle pulling from 5+ subsystems
/// (commands, agents, auth/account, config, rate-limit state). Rather than
/// plumbing each source through `SdkServerState` as a separate field, the
/// server takes one trait object that encapsulates all of them. The concrete
/// impl lives in `coco-cli` where every source is already imported; tests
/// can substitute a mock.
///
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
}

// ---------------------------------------------------------------------------
// Server + session state
// ---------------------------------------------------------------------------

/// Shared server state carried across ClientRequests within a single
/// stdio session. Only one concurrent session per server — a single
/// `currentSession` slot per server process.
pub struct SdkServerState {
    /// Active session if any. Set by `session/start`, cleared by
    /// `session/archive` or when the transport closes.
    pub session: RwLock<Option<SessionHandle>>,
    /// The runner that executes turns. Defaulted to `NotImplementedRunner`.
    /// Stored behind `RwLock` so `SdkServer::set_turn_runner()` can
    /// install a real runner after the state is already shared (used
    /// by the approval-bridge wiring path where the bridge needs a
    /// reference to the live state before the runner is constructed).
    pub turn_runner: RwLock<Arc<dyn TurnRunner>>,
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
    ///
    /// Populated by [`SdkServerState::send_server_request`] when an
    /// outbound request is written to the transport; drained by the
    /// dispatcher's `handle_message` when the matching response arrives.
    pub pending_server_requests: Mutex<HashMap<RequestId, oneshot::Sender<JsonRpcMessage>>>,
    /// Monotonic counter for issuing unique request IDs for outbound
    /// ServerRequests. Uses negative integers to avoid colliding with
    /// client-issued IDs (which are typically non-negative).
    pub next_server_request_id: AtomicI64,
    /// Transport handle shared with the dispatcher. Populated by
    /// `SdkServer::run()` at startup; used by the approval bridge and
    /// other ServerRequest-emitting code paths. `None` in tests that
    /// construct `SdkServerState` directly.
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
    /// pointing at `~/.coco/sessions`; in-memory tests that don't
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
    ///
    /// When `None`, the MCP lifecycle handlers respond with
    /// `INVALID_REQUEST` ("MCP manager not enabled").
    pub mcp_manager: RwLock<Option<Arc<Mutex<coco_mcp::McpConnectionManager>>>>,
    /// Optional [`InitializeBootstrap`] provider used by `handle_initialize`
    /// to populate `commands`, `agents`, `account`, `output_style`, etc.
    /// When `None`, the handler returns empty / default values for those
    /// fields so the wire format stays TS-conformant.
    pub initialize_bootstrap: RwLock<Option<Arc<dyn InitializeBootstrap>>>,
    /// Whether the SDK client opted into per-spawn periodic
    /// AgentSummary timers via `initialize { agentProgressSummaries: true }`.
    /// TS parity: `bootstrap/state.ts:1077` `getSdkAgentProgressSummariesEnabled`.
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
    ///
    /// TS parity: `cli/print.ts:4588-4600` — mid-session SDK switches
    /// to `bypassPermissions` are rejected with an explicit error
    /// when `isBypassPermissionsModeAvailable` is false.
    pub bypass_permissions_available: std::sync::atomic::AtomicBool,
    /// Process-shared `SessionRuntime`. Set by `run_sdk_mode` at
    /// startup (same Arc threaded into `QueryEngineRunner`). Read by
    /// `handle_session_start` to call `runtime.start_new_session()`
    /// when an SDK client cycles `session/archive` → `session/start`,
    /// so the new session sees fresh `FileReadState`,
    /// `SessionMemoryService` paths, file-history sink target, and
    /// cache-break detector baseline. `None` only in tests that don't
    /// wire a runtime.
    pub session_runtime: RwLock<Option<Arc<crate::session_runtime::SessionRuntime>>>,

    /// Agent definitions pushed via `initialize.agents` (TS
    /// `controlSchemas.ts:68 z.record(z.string(),
    /// AgentDefinitionSchema)`). Stashed here at `initialize` time and
    /// drained when the per-session `AgentDefinitionStore` is built so
    /// SDK-supplied agents land in the catalog as `AgentSource::FlagSettings`
    /// entries (TS parity: `parseAgentsFromJson(_, 'flagSettings')` at
    /// `cli/print.ts:4382`).
    ///
    /// `RwLock` (not `Mutex`) because `agents()` in `CliInitializeBootstrap`
    /// reads concurrently with other initialize-time accessors.
    pub pending_sdk_agents: RwLock<Vec<coco_types::AgentDefinition>>,

    /// Last `RegisterMcpToolsReport` per MCP server (v4.2). Written by the
    /// register call sites; read by `handle_mcp_status` to source the
    /// registered `tool_count` + skipped / tombstoned tools. Cleared on
    /// disconnect; bounded by the number of distinct server names seen.
    pub mcp_registration_reports: RwLock<HashMap<String, coco_tools::RegisterMcpToolsReport>>,
}

impl Default for SdkServerState {
    fn default() -> Self {
        Self {
            session: RwLock::new(None),
            turn_runner: RwLock::new(Arc::new(NotImplementedRunner) as Arc<dyn TurnRunner>),
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
            agent_progress_summaries_enabled: std::sync::atomic::AtomicBool::new(false),
            bypass_permissions_available: std::sync::atomic::AtomicBool::new(false),
            session_runtime: RwLock::new(None),
            pending_sdk_agents: RwLock::new(Vec::new()),
            mcp_registration_reports: RwLock::new(HashMap::new()),
        }
    }
}

impl SdkServerState {
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
    ///
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
    ///
    /// Generates a fresh monotonically-decreasing `RequestId` (starting
    /// at -1), registers an oneshot in `pending_server_requests`, writes
    /// the `JsonRpcRequest` onto the transport, and awaits the receiver.
    /// The dispatcher's inbound-message handler wakes the receiver when
    /// the client replies with a matching `Response`/`Error`.
    ///
    /// Returns:
    /// - `Ok(JsonRpcMessage::Response(r))` — client replied successfully
    /// - `Ok(JsonRpcMessage::Error(e))` — client replied with an error
    /// - `Err(...)` — transport send failed or the oneshot was dropped
    ///   (e.g. the transport closed before the client replied)
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
        //
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
        // guarantee for any call made between `SdkServer::new()` and
        // `SdkServer::run()`. Callers must wait for `run()` to have
        // populated `outbound_tx`; the only code path that hit the
        // old fallback was tests, which now wait explicitly.
        let req = JsonRpcRequest {
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
    /// request with the matching `request_id`, if any. Called by the
    /// dispatcher when it reads a message from the transport.
    ///
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
            .finish()
    }
}

/// Handle for an active SDK session.
#[derive(Debug)]
pub struct SessionHandle {
    pub session_id: String,
    pub cwd: String,
    pub model: String,
    /// Session-scoped permission mode override. When `None`, turns use
    /// the CLI-default mode; `control/setPermissionMode` sets this.
    pub permission_mode: Option<coco_types::PermissionMode>,
    /// Session-scoped thinking level override.
    /// `control/setThinking` sets this.
    pub thinking_level: Option<coco_types::ThinkingLevel>,
    /// Session-scoped environment variable overrides. Mutated by
    /// `control/updateEnv`; applied to tool invocations when wired.
    pub env_overrides: std::collections::HashMap<String, String>,
    /// Cancel token for the currently-running turn (if any).
    /// Set by `turn/start`, cleared when the turn completes.
    /// `turn/interrupt` looks this up and calls `.cancel()`.
    pub active_turn_cancel: Option<CancellationToken>,
    /// `JoinHandle` for the currently-running turn's runner task.
    /// `session/archive` takes this out (after cancelling) and awaits it
    /// to ensure every event the runner emits is flushed through the
    /// forwarder before the aggregated `SessionResult` is sent, so the
    /// client sees the archive event last.
    pub active_turn_task: Option<tokio::task::JoinHandle<()>>,
    /// `JoinHandle` for the currently-running turn's event forwarder
    /// task. Paired with `active_turn_task` — archive awaits both, in
    /// order (runner first, forwarder second), so every per-turn event
    /// has been written to `notif_tx` before the aggregated result.
    pub active_turn_forwarder: Option<tokio::task::JoinHandle<()>>,
    /// Monotonic counter for issuing turn IDs within this session.
    pub turn_counter: i32,
    /// Wall-clock timestamp when the session was created (for duration_ms).
    pub started_at: std::time::Instant,
    /// Per-session aggregated stats accumulated across every `turn/start`.
    /// Populated by the event forwarder when it intercepts per-turn
    /// `SessionResult` notifications from the engine. Emitted back to the
    /// client as a single `SessionResult` when the session is archived.
    pub stats: SessionStats,
    /// Cumulative message history across every `turn/start` in this
    /// session. Used by `QueryEngineRunner` to thread context between
    /// turns: the runner locks this, builds combined messages
    /// `prior_history + [new_user_msg]`, calls
    /// `QueryEngine::run_with_messages`, then replaces the contents
    /// with `QueryResult.final_messages`. The `Arc<Mutex<>>` wrapping
    /// lets the runner's detached turn task mutate it without holding
    /// the session write-lock for the whole turn.
    pub history: Arc<Mutex<Vec<std::sync::Arc<coco_messages::Message>>>>,

    /// Session-scoped `ToolAppState` — TS parity:
    /// `appState.toolPermissionContext` and the plan-mode latches.
    /// Created once at session/start, attached to every turn's engine
    /// via `with_app_state`, and mutated by
    /// `control/setPermissionMode` so Shift+Tab / SDK mode toggles
    /// propagate to the engine's next `create_tool_context` read.
    /// Before this wiring, `control/setPermissionMode` was a dead
    /// API (wrote only `SessionHandle.permission_mode`, which no
    /// reader consumed).
    pub app_state: Arc<RwLock<coco_types::ToolAppState>>,
}

impl SessionHandle {
    pub(super) fn new(session_id: String, cwd: String, model: String) -> Self {
        Self {
            session_id,
            cwd,
            model,
            permission_mode: None,
            thinking_level: None,
            env_overrides: std::collections::HashMap::new(),
            active_turn_cancel: None,
            active_turn_task: None,
            active_turn_forwarder: None,
            turn_counter: 0,
            started_at: std::time::Instant::now(),
            stats: SessionStats::default(),
            history: Arc::new(Mutex::new(Vec::new())),
            app_state: Arc::new(RwLock::new(coco_types::ToolAppState::default())),
        }
    }

    /// Build a narrow [`TurnHandoff`] for this session — avoids deep-
    /// cloning `stats` / `env_overrides` / `permission_denials` just to
    /// hand a turn to the runner.
    pub fn handoff(&self) -> TurnHandoff {
        TurnHandoff {
            session_id: self.session_id.clone(),
            cwd: self.cwd.clone(),
            model: self.model.clone(),
            history: Arc::clone(&self.history),
            app_state: Arc::clone(&self.app_state),
            permission_mode: self.permission_mode,
        }
    }
}

/// Aggregated per-session stats, mirrored from per-turn `SessionResult`
/// notifications emitted by `QueryEngine::run_with_events`.
///
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

/// Per-request context passed to handlers.
pub struct HandlerContext {
    /// Channel for forwarding CoreEvent notifications to the transport.
    /// Handlers that spawn a QueryEngine pass this as the engine's
    /// `event_tx`. Single-shot handlers (e.g., `initialize`) rarely use
    /// it; long-running handlers (e.g., `turn/start`) emit events here.
    pub notif_tx: mpsc::Sender<OutboundMessage>,

    /// Shared server state across requests (session slot).
    pub state: Arc<SdkServerState>,
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
///
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
        ClientRequest::SessionArchive(params) => {
            session::handle_session_archive(params, &ctx).await
        }

        // === Turn control ===
        ClientRequest::TurnStart(params) => turn::handle_turn_start(params, &ctx).await,
        ClientRequest::TurnInterrupt => turn::handle_turn_interrupt(&ctx).await,

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
        ClientRequest::SetPermissionMode(params) => {
            runtime::handle_set_permission_mode(params, &ctx).await
        }
        ClientRequest::SetThinking(params) => runtime::handle_set_thinking(params, &ctx).await,
        ClientRequest::StopTask(params) => runtime::handle_stop_task(params, &ctx).await,
        ClientRequest::RewindFiles(params) => rewind::handle_rewind_files(params, &ctx).await,
        ClientRequest::UpdateEnv(params) => runtime::handle_update_env(params, &ctx).await,

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
        ClientRequest::ConfigApplyFlags(params) => {
            runtime::handle_config_apply_flags(params, &ctx).await
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
