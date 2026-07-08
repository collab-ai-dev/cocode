//! SDK server dispatch loop.
//!
//! The `SdkServer` reads `JsonRpcMessage` requests from a transport,
//! dispatches them to per-method handlers, and writes responses +
//! forwarded CoreEvent notifications back to the transport.
//!
//! The dispatch loop reads stdin, routes control requests, and enqueues
//! messages to stdout.

use std::collections::HashMap;
use std::sync::Arc;

use coco_hub_connector::HubConnectorSender;
use coco_query::StreamAccumulator;
use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::JSONRPC_VERSION;
use coco_types::JsonRpcNotification;
use coco_types::ServerNotification;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::info;
use tracing::warn;

use crate::sdk_server::handlers::SdkServerState;
use crate::sdk_server::handlers::TurnRunner;
use crate::sdk_server::outbound::OutboundMessage;
use crate::sdk_server::transport::SdkTransport;

/// The SDK server — owns the transport, dispatches ClientRequests, and
/// forwards CoreEvent notifications to the client.
///
/// Lifecycle:
/// 1. Construct with `SdkServer::new(transport)`.
/// 2. Create a `JsonRpcAdapterConnection`.
/// 3. Call [`Self::run_app_server_connection`]. It loops until the transport
///    closes and forwards notifications from the agent loop through the SDK
///    single-writer serializer.
pub struct SdkServer {
    transport: Arc<dyn SdkTransport>,
    /// Shared session state across dispatched requests.
    state: Arc<SdkServerState>,
    /// Optional external-event channels merged into the main
    /// `notif_tx` stream inside [`Self::run_app_server_connection`]. Each entry is a
    /// `Receiver<CoreEvent>` produced by a long-running subsystem
    /// (e.g. the plugin file watcher) that wants its events to land
    /// in the SDK NDJSON output alongside engine-emitted notifications.
    /// `Mutex` for `Take`-able interior mutability — the AppServer bridge
    /// drains it.
    /// Modeled as merged channels so external subsystems can push
    /// events into the notification system.
    external_notifications: std::sync::Mutex<Vec<mpsc::Receiver<CoreEvent>>>,
    hub_connector: Option<HubConnectorSender>,
}

impl SdkServer {
    /// Create a new SDK server bound to a transport.
    ///
    /// The transport is published onto `state.transport` immediately so
    /// code paths that read it (e.g. [`crate::sdk_server::SdkPermissionBridge`])
    /// see a populated slot without waiting for
    /// [`Self::run_app_server_connection`] to start.
    /// This avoids a startup race where a bridge consulted between
    /// `new()` and `run_app_server_connection()` would erroneously see `None`.
    pub fn new(transport: Arc<dyn SdkTransport>) -> Self {
        let state = Arc::new(SdkServerState::default());
        // Pre-populate the transport slot. At construction time nothing
        // else has a lock on the state, so `try_write` is guaranteed to
        // succeed. We panic if it doesn't — that would indicate a
        // programmer error (e.g. the state was pre-shared).
        {
            let Ok(mut slot) = state.transport.try_write() else {
                panic!("SdkServer::new: state was already locked at construction time");
            };
            *slot = Some(transport.clone());
        }
        Self {
            transport,
            state,
            external_notifications: std::sync::Mutex::new(Vec::new()),
            hub_connector: None,
        }
    }

    /// Register an external notification source whose events should be
    /// forwarded to the SDK NDJSON output alongside engine-emitted
    /// notifications. Used by the plugin file watcher so SDK clients
    /// receive `plugins/changed` like TUI clients do.
    pub fn with_external_notifications(self, rx: mpsc::Receiver<CoreEvent>) -> Self {
        self.external_notifications
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(rx);
        self
    }

    /// Clone SDK-visible protocol notifications into the Event Hub connector.
    ///
    /// The SDK writer remains the single owner of NDJSON output ordering; this
    /// side channel stamps the same protocol notifications with the active SDK
    /// session id for Hub egress without changing SDK wire behavior.
    pub fn with_hub_connector_sender(mut self, sender: HubConnectorSender) -> Self {
        self.hub_connector = Some(sender);
        self
    }

    /// Inject a custom [`TurnRunner`] synchronously during builder
    /// construction. Mutates the existing shared state in place (via
    /// `try_write`). Call this before [`Self::run_app_server_connection`] to
    /// wire the production `QueryEngine`-backed runner, or to install a mock
    /// runner in tests. Without this, `turn/start` fails with
    /// `NotImplementedRunner`.
    ///
    /// Panics if the `turn_runner` lock is already held — that would
    /// indicate a programmer error (the state was pre-shared and a
    /// reader is active during construction).
    pub fn with_turn_runner(self, runner: Arc<dyn TurnRunner>) -> Self {
        let Ok(mut slot) = self.state.turn_runner.try_write() else {
            panic!("with_turn_runner: state was already locked at construction time");
        };
        *slot = runner;
        drop(slot);
        self
    }

    /// Install a disk-backed [`coco_session::SessionManager`] so the
    /// `session/list`, `session/read`, `session/resume` handlers can
    /// browse and resume historical sessions. Without this, those
    /// handlers reply with `METHOD_NOT_FOUND`.
    pub fn with_session_manager(self, manager: Arc<coco_session::SessionManager>) -> Self {
        let Ok(mut slot) = self.state.session_manager.try_write() else {
            panic!("with_session_manager: state was already locked at construction time");
        };
        *slot = Some(manager);
        drop(slot);
        self
    }

    /// Install a [`coco_context::FileHistoryState`] + config home so
    /// the `control/rewindFiles` handler can preview and apply file
    /// rewinds. Without this, the handler errors with
    /// `INVALID_REQUEST` ("file history not enabled").
    pub fn with_file_history(
        self,
        history: Arc<tokio::sync::RwLock<coco_context::FileHistoryState>>,
        config_home: std::path::PathBuf,
    ) -> Self {
        {
            let Ok(mut slot) = self.state.file_history.try_write() else {
                panic!("with_file_history: state was already locked at construction time");
            };
            *slot = Some(history);
        }
        {
            let Ok(mut slot) = self.state.file_history_config_home.try_write() else {
                panic!("with_file_history: state was already locked at construction time");
            };
            *slot = Some(config_home);
        }
        self
    }

    /// Install an [`coco_mcp::McpConnectionManager`] so the
    /// `mcp/setServers`, `mcp/reconnect`, `mcp/toggle` handlers can
    /// register configs and drive connection lifecycle. Without this,
    /// those handlers reply with `INVALID_REQUEST` ("MCP manager not
    /// enabled").
    pub fn with_mcp_manager(
        self,
        manager: Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>,
    ) -> Self {
        let Ok(mut slot) = self.state.mcp_manager.try_write() else {
            panic!("with_mcp_manager: state was already locked at construction time");
        };
        *slot = Some(manager);
        drop(slot);
        self
    }

    /// Install an [`InitializeBootstrap`] provider so `handle_initialize`
    /// returns real data (commands, agents, account, output styles) instead
    /// of empty / default values. Without this, `initialize` still succeeds
    /// with a conformant shape but empty lists.
    pub fn with_initialize_bootstrap(
        self,
        bootstrap: Arc<dyn crate::sdk_server::handlers::InitializeBootstrap>,
    ) -> Self {
        let Ok(mut slot) = self.state.initialize_bootstrap.try_write() else {
            panic!("with_initialize_bootstrap: state was already locked at construction time");
        };
        *slot = Some(bootstrap);
        drop(slot);
        self
    }

    /// Install the cwd captured by the process entrypoint before requests
    /// arrive. SDK handlers use it when no active session/runtime exists yet.
    pub fn with_startup_cwd(self, cwd: std::path::PathBuf) -> Self {
        let Ok(mut slot) = self.state.startup_cwd.try_write() else {
            panic!("with_startup_cwd: state was already locked at construction time");
        };
        *slot = Some(cwd);
        drop(slot);
        self
    }

    /// Install the process-shared [`SessionHandle`]. Required so
    /// `handle_session_start` can call `runtime.retarget_for_new_session()`
    /// when an SDK client cycles `session/archive` → `session/start`.
    /// Without this, sequential SDK sessions reuse the prior session's
    /// `FileReadState`, `SessionMemoryService` paths, file-history sink
    /// session id, and cache-break baseline — surfacing as @mention
    /// dedup leakage, memory writes to wrong directory, and false-
    /// positive cache break alerts on the first turn of session 2.
    pub fn with_session_handle(self, session: crate::session_runtime::SessionHandle) -> Self {
        crate::sdk_server::sdk_hooks::install_runtime_callback(self.state.clone(), &session);
        let Ok(mut slot) = self.state.session_runtime.try_write() else {
            panic!("with_session_handle: state was already locked at construction time");
        };
        *slot = Some(session);
        drop(slot);
        self
    }

    /// Asynchronously replace the installed [`TurnRunner`]. Used by
    /// code paths that need to construct the runner after cloning the
    /// shared state (e.g. the approval-bridge wiring in
    /// `run_sdk_mode`, where the bridge needs a reference to live
    /// state before the runner exists).
    pub async fn set_turn_runner(&self, runner: Arc<dyn TurnRunner>) {
        let mut slot = self.state.turn_runner.write().await;
        *slot = runner;
    }

    /// Access the underlying transport — used by code paths that need
    /// to issue outbound `ServerRequest` messages (e.g. the approval
    /// bridge).
    pub fn transport(&self) -> Arc<dyn SdkTransport> {
        self.transport.clone()
    }

    /// Access the shared state. Used by tests (and in the future, the CLI
    /// wiring) to register pending approvals / user inputs before sending
    /// the matching ServerRequest on the wire.
    pub fn state(&self) -> Arc<SdkServerState> {
        self.state.clone()
    }

    /// Run this SDK server through the AppServer JSON-RPC adapter bridge.
    ///
    /// This preserves the SDK transport, shared `SdkServerState`, MCP route
    /// setup, external notification forwarding, and single-writer serializer,
    /// while delegating JSON-RPC request ownership to `coco-app-server`.
    pub async fn run_app_server_connection(
        &self,
        connection: coco_app_server::JsonRpcAdapterConnection<
            crate::sdk_server::LocalAppSessionHandle,
        >,
    ) -> Result<coco_app_server::DisconnectOutcome, crate::sdk_server::SdkAppServerBridgeError>
    {
        info!("SdkServer starting AppServer bridge dispatch loop");
        let external_notifications = {
            let mut guard = self
                .external_notifications
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *guard)
        };
        crate::sdk_server::app_server_bridge::run_app_server_sdk_state_over_sdk_transport_with_external_notifications_and_hub_connector(
            connection,
            self.transport.clone(),
            self.state.clone(),
            external_notifications,
            self.hub_connector.clone(),
        )
        .await
    }
}

#[derive(Clone)]
pub(crate) struct SdkHubEgress {
    state: Arc<SdkServerState>,
    sender: HubConnectorSender,
}

impl SdkHubEgress {
    pub(crate) fn new(state: Arc<SdkServerState>, sender: HubConnectorSender) -> Self {
        Self { state, sender }
    }

    async fn enqueue_notification(
        &self,
        next_session_seq: &mut HashMap<SessionId, i64>,
        notification: &ServerNotification,
    ) {
        let Some(session_id) = current_sdk_session_id(&self.state).await else {
            warn!("dropping SDK Event Hub notification without an active session");
            return;
        };
        let seq_session_id = session_id.clone();
        let envelope = SessionEnvelope::stamp(
            session_id,
            None,
            CoreEvent::Protocol(notification.clone()),
            || {
                let next = next_session_seq.entry(seq_session_id).or_insert(1);
                let seq = *next;
                *next += 1;
                seq
            },
        );
        if let Err(error) = self.sender.try_enqueue(envelope) {
            warn!(%error, "dropping SDK Event Hub notification from connector queue");
        }
    }
}

async fn current_sdk_session_id(state: &SdkServerState) -> Option<SessionId> {
    if let Some(session) = state.session.read().await.as_ref() {
        return Some(session.session_id.clone());
    }
    let runtime = state.session_runtime.read().await.clone();
    if let Some(runtime) = runtime {
        return Some(runtime.current_typed_session_id().await);
    }
    None
}

/// Spawn the SDK's single-writer transport serializer.
///
/// Every outbound notification, reply, and server request is enqueued into one
/// channel so wire order matches enqueue order. This is critical for
/// `session/archive`, where the aggregated `SessionResult` event must land
/// before the archive JSON-RPC reply, and for SDK MCP/hook callbacks that share
/// the same stdout/WebSocket stream with turn events.
pub(crate) fn spawn_sdk_outbound_writer(
    transport: Arc<dyn SdkTransport>,
    mut outbound_rx: mpsc::Receiver<OutboundMessage>,
    hub_egress: Option<SdkHubEgress>,
) -> tokio::task::JoinHandle<()> {
    // Tag every event emitted from the writer task with `sdk_writer` so
    // downstream log filters can trace "what did the SDK see, in what order"
    // without cross-referencing task IDs.
    let writer_span = tracing::info_span!("sdk_writer");
    tokio::spawn(tracing::Instrument::instrument(
        async move {
            // Per-turn StreamAccumulator. Converts AgentStreamEvent sequences
            // into semantic ServerNotification::ItemStarted/Updated/Completed
            // + AgentMessageDelta/ReasoningDelta protocol events. Reset on
            // each TurnStarted, flushed on TurnCompleted/Failed/Interrupted.
            let mut accumulator: Option<StreamAccumulator> = None;
            // Buffer stream events that arrive before TurnStarted.
            const PRE_TURN_BUFFER_CAP: usize = 64;
            let mut pre_turn_buffer: Vec<AgentStreamEvent> = Vec::new();
            let mut next_session_seq: HashMap<SessionId, i64> = HashMap::new();

            async fn send_notif(
                transport: &dyn SdkTransport,
                hub_egress: Option<&SdkHubEgress>,
                next_session_seq: &mut HashMap<SessionId, i64>,
                notif: &ServerNotification,
            ) -> bool {
                if let Some(hub_egress) = hub_egress {
                    hub_egress
                        .enqueue_notification(next_session_seq, notif)
                        .await;
                }
                if let Err(e) = transport.send_notification(notif).await {
                    warn!(error = %e, "notification forward failed");
                    return false;
                }
                true
            }

            async fn send_accumulated(
                transport: &dyn SdkTransport,
                hub_egress: Option<&SdkHubEgress>,
                next_session_seq: &mut HashMap<SessionId, i64>,
                notifications: Vec<ServerNotification>,
            ) -> bool {
                for sn in notifications {
                    if !send_notif(transport, hub_egress, next_session_seq, &sn).await {
                        return false;
                    }
                }
                true
            }

            while let Some(outbound) = outbound_rx.recv().await {
                match outbound {
                    OutboundMessage::CoreEvent(event) => match *event {
                        CoreEvent::Protocol(notif) => {
                            match &notif {
                                ServerNotification::TurnStarted(p) => {
                                    let turn_id = p.turn_id.as_str().to_string();
                                    let mut acc = StreamAccumulator::new(turn_id);
                                    let buffered: Vec<_> = pre_turn_buffer
                                        .drain(..)
                                        .flat_map(|evt| acc.process(evt))
                                        .collect();
                                    if !send_accumulated(
                                        &*transport,
                                        hub_egress.as_ref(),
                                        &mut next_session_seq,
                                        buffered,
                                    )
                                    .await
                                    {
                                        break;
                                    }
                                    accumulator = Some(acc);
                                }
                                ServerNotification::TurnEnded(_) => {
                                    if let Some(ref mut acc) = accumulator {
                                        let flushed = acc.flush();
                                        if !send_accumulated(
                                            &*transport,
                                            hub_egress.as_ref(),
                                            &mut next_session_seq,
                                            flushed,
                                        )
                                        .await
                                        {
                                            break;
                                        }
                                    }
                                    accumulator = None;
                                    pre_turn_buffer.clear();
                                }
                                _ => {}
                            }
                            if !send_notif(
                                &*transport,
                                hub_egress.as_ref(),
                                &mut next_session_seq,
                                &notif,
                            )
                            .await
                            {
                                break;
                            }
                        }
                        CoreEvent::Stream(stream_evt) => {
                            let notifications = if let Some(ref mut acc) = accumulator {
                                acc.process(stream_evt)
                            } else {
                                if pre_turn_buffer.len() >= PRE_TURN_BUFFER_CAP {
                                    warn!(
                                        metric = "pre_turn_buffer_overflow",
                                        cap = PRE_TURN_BUFFER_CAP,
                                        "pre-turn buffer full, dropping stream event"
                                    );
                                } else {
                                    debug!("stream event before TurnStarted; buffering");
                                    pre_turn_buffer.push(stream_evt);
                                }
                                Vec::new()
                            };
                            if !send_accumulated(
                                &*transport,
                                hub_egress.as_ref(),
                                &mut next_session_seq,
                                notifications,
                            )
                            .await
                            {
                                break;
                            }
                        }
                        CoreEvent::Tui(_) => {}
                    },
                    OutboundMessage::JsonRpc(msg) => {
                        if let Err(e) = transport.send(msg).await {
                            warn!(error = %e, "json-rpc forward failed");
                            break;
                        }
                    }
                }
            }
            debug!("transport writer exited");
        },
        writer_span,
    ))
}

// ---------------------------------------------------------------------------
// CoreEvent → JsonRpcNotification
// ---------------------------------------------------------------------------

/// Translate a `CoreEvent` into a `JsonRpcNotification` suitable for the
/// wire. Returns `None` for `CoreEvent::Tui(_)` (dropped by non-TUI
/// consumers) and `CoreEvent::Stream(_)` (handled by the writer task's
/// `StreamAccumulator`, not this function).
///
/// See `event-system-design.md` §12.
///
/// Only used in tests — the production writer task handles dispatch inline.
#[cfg(test)]
fn core_event_to_notification(event: CoreEvent) -> Option<JsonRpcNotification> {
    match event {
        CoreEvent::Protocol(notif) => server_notification_to_jsonrpc(notif),
        CoreEvent::Stream(_) => None,
        CoreEvent::Tui(_) => None,
    }
}

/// Serialize a `ServerNotification` as a `JsonRpcNotification` directly.
/// Exposed for handlers that want to emit synthetic protocol notifications
/// without going through CoreEvent.
///
/// Extracts both `method` and `params` from the serde-serialized `Value`
/// so serde's `#[serde(tag = "method")]` stays the single source of truth
/// for the wire envelope.
pub fn server_notification_to_jsonrpc(notif: ServerNotification) -> Option<JsonRpcNotification> {
    match serde_json::to_value(notif).ok()? {
        Value::Object(mut map) => {
            let method = match map.remove("method")? {
                Value::String(s) => s,
                _ => return None,
            };
            let params = map.remove("params").unwrap_or(Value::Null);
            Some(JsonRpcNotification {
                jsonrpc: JSONRPC_VERSION.into(),
                method,
                params,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "dispatcher.test.rs"]
mod tests;
