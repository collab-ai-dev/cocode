//! SDK server dispatch loop.
//!
//! The `SdkServer` routes AppServer JSON-RPC frames from a transport,
//! dispatches them to per-method handlers, and writes responses plus
//! forwarded CoreEvent notifications back through the ordered writer.
//!
//! The dispatch loop reads stdin, routes control requests, and enqueues
//! messages to stdout.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use coco_app_server::AppServer;
use coco_app_server::SessionSeqAllocator;
use coco_app_server_transport::JsonRpcFrame;
use coco_app_server_transport::JsonRpcNotification as TransportJsonRpcNotification;
use coco_hub_connector::HubConnectorSender;
use coco_query::StreamAccumulator;
use coco_types::AgentId;
use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::JSONRPC_VERSION;
use coco_types::JsonRpcNotification as LegacyJsonRpcNotification;
use coco_types::ServerNotification;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SurfaceId;
use coco_types::TurnId;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::info;
use tracing::warn;

use crate::sdk_server::handlers::SdkServerState;
use crate::sdk_server::handlers::TurnRunner;
use crate::sdk_server::outbound::OutboundMessage;
use crate::sdk_server::outbound::event_agent_id;
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
    app_server_turn_drain_timeout: Duration,
}

impl SdkServer {
    /// Create a new SDK server bound to a transport.
    ///
    /// The transport is published onto SDK connection state immediately so
    /// code paths that read it (e.g. [`crate::sdk_server::SdkPermissionBridge`])
    /// see a populated slot without waiting for
    /// [`Self::run_app_server_connection`] to start.
    /// This avoids a startup race where a bridge consulted between
    /// `new()` and `run_app_server_connection()` would erroneously see `None`.
    pub fn new(transport: Arc<dyn SdkTransport>) -> Self {
        let state = Arc::new(SdkServerState::default());
        state.install_sdk_transport_for_startup(transport.clone());
        Self {
            transport,
            state,
            external_notifications: std::sync::Mutex::new(Vec::new()),
            hub_connector: None,
            app_server_turn_drain_timeout:
                crate::sdk_server::app_server_bridge::APP_SERVER_TURN_DRAIN_TIMEOUT,
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

    pub fn with_app_server_turn_drain_timeout(mut self, timeout: Duration) -> Self {
        self.app_server_turn_drain_timeout = timeout;
        self
    }

    /// Inject a custom [`TurnRunner`] synchronously during builder
    /// construction. Mutates the existing shared state in place (via
    /// `try_write`). Call this before [`Self::run_app_server_connection`] to
    /// wire the production `QueryEngine`-backed runner, or to install a mock
    /// runner in tests. Without this, `turn/start` fails with
    /// `NotImplementedRunner`.
    ///
    /// Panics if the turn-runner slot is already held — that would
    /// indicate a programmer error (the state was pre-shared and a
    /// reader is active during construction).
    pub fn with_turn_runner(self, runner: Arc<dyn TurnRunner>) -> Self {
        self.state.install_turn_runner_for_startup(runner);
        self
    }

    /// Install a disk-backed [`coco_session::SessionManager`] so the
    /// `session/list`, `session/read`, `session/resume` handlers can
    /// browse and resume historical sessions. Without this, those
    /// handlers reply with `METHOD_NOT_FOUND`.
    pub fn with_session_manager(self, manager: Arc<coco_session::SessionManager>) -> Self {
        self.state.install_session_manager_for_startup(manager);
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
        self.state
            .install_file_history_for_startup(history, config_home);
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
        self.state.install_mcp_manager_for_startup(manager);
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
        self.state
            .install_initialize_bootstrap_for_startup(bootstrap);
        self
    }

    /// Install the cwd captured by the process entrypoint before requests
    /// arrive. SDK handlers use it when no active session/runtime exists yet.
    pub fn with_startup_cwd(self, cwd: std::path::PathBuf) -> Self {
        self.state.install_startup_cwd(cwd);
        self
    }

    /// Install the process-shared [`SessionHandle`]. Production SDK
    /// `session/start` / `session/resume` must pair this with a
    /// [`crate::sdk_server::RuntimeReplacementContext`] so new client sessions
    /// build replacement runtimes instead of rotating this one in place.
    pub fn with_session_handle(self, session: crate::session_runtime::SessionHandle) -> Self {
        crate::sdk_server::sdk_hooks::install_runtime_callback(self.state.clone(), &session);
        self.state.install_session_runtime_for_startup(session);
        self
    }

    /// Asynchronously replace the installed [`TurnRunner`]. Used by
    /// code paths that need to construct the runner after cloning the
    /// shared state (e.g. the approval-bridge wiring in
    /// `run_sdk_mode`, where the bridge needs a reference to live
    /// state before the runner exists).
    pub async fn set_turn_runner(&self, runner: Arc<dyn TurnRunner>) {
        self.state.install_turn_runner(runner).await;
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
            self.app_server_turn_drain_timeout,
        )
        .await
    }
}

/// Spawn the SDK's single-writer transport serializer.
///
/// Every outbound notification, reply, and server request is enqueued into one
/// channel so wire order matches enqueue order. This is critical for
/// `session/archive`, where the aggregated `SessionResult` event must land
/// before the archive JSON-RPC reply, and for SDK MCP/hook callbacks that share
/// the same stdout/WebSocket stream with turn events.
pub(crate) fn spawn_sdk_outbound_writer<H>(
    transport: Arc<dyn SdkTransport>,
    mut outbound_rx: mpsc::Receiver<OutboundMessage>,
    app_server: Arc<AppServer<H>>,
    session_seq: Arc<SessionSeqAllocator>,
    hub_connector: Option<HubConnectorSender>,
) -> tokio::task::JoinHandle<()>
where
    H: Clone + Send + Sync + 'static,
{
    // Tag every event emitted from the writer task with `sdk_writer` so
    // downstream log filters can trace "what did the SDK see, in what order"
    // without cross-referencing task IDs.
    let writer_span = tracing::info_span!("sdk_writer");
    tokio::spawn(tracing::Instrument::instrument(
        async move {
            let mut renderer = SdkEventRenderer::default();
            while let Some(outbound) = outbound_rx.recv().await {
                match outbound {
                    OutboundMessage::SessionEvent {
                        session_id,
                        event,
                        routed,
                    } => {
                        let agent_id = event_agent_id(&event);
                        let seq_session_id = session_id.clone();
                        let envelope = SessionEnvelope::stamp(session_id, agent_id, *event, || {
                            session_seq.next(&seq_session_id)
                        });
                        let hub_envelope = envelope.clone();
                        app_server.route_envelope(envelope);
                        if let Some(hub_connector) = &hub_connector
                            && let Err(error) = hub_connector.try_enqueue(hub_envelope)
                        {
                            warn!(%error, "dropping SDK AppServer event from Hub connector queue");
                        }
                        if let Some(routed) = routed {
                            let _ = routed.send(());
                        }
                    }
                    OutboundMessage::ProcessEvent(event) => {
                        let notification = event.into_notification();
                        if let Err(error) = transport.send_notification(&notification).await {
                            warn!(%error, "process notification forward failed");
                            break;
                        }
                    }
                    OutboundMessage::JsonRpcFrame(frame) => match renderer.render_frame(frame) {
                        Ok(frames) => {
                            let mut failed = false;
                            for frame in frames {
                                if let Err(error) = transport.send_frame(frame).await {
                                    warn!(%error, "json-rpc frame forward failed");
                                    failed = true;
                                    break;
                                }
                            }
                            if failed {
                                break;
                            }
                        }
                        Err((error, frame)) => {
                            warn!(%error, "failed to render routed SDK event; forwarding canonical frame");
                            if let Err(error) = transport.send_frame(frame).await {
                                warn!(%error, "json-rpc frame forward failed");
                                break;
                            }
                        }
                    },
                }
            }
            debug!("transport writer exited");
        },
        writer_span,
    ))
}

#[derive(Default)]
struct SdkEventRenderer {
    accumulators: HashMap<SessionId, StreamAccumulator>,
    pre_turn: HashMap<SessionId, Vec<AgentStreamEvent>>,
}

impl SdkEventRenderer {
    const PRE_TURN_BUFFER_CAP: usize = 64;

    fn render_frame(
        &mut self,
        frame: JsonRpcFrame,
    ) -> Result<Vec<JsonRpcFrame>, (serde_json::Error, JsonRpcFrame)> {
        let JsonRpcFrame::Notification(notification) = &frame else {
            return Ok(vec![frame]);
        };
        if notification.method != "session/event" {
            return Ok(vec![frame]);
        }
        match self.render_session_event(notification) {
            Ok(frames) => Ok(frames),
            Err(error) => Err((error, frame)),
        }
    }

    fn render_session_event(
        &mut self,
        notification: &TransportJsonRpcNotification,
    ) -> Result<Vec<JsonRpcFrame>, serde_json::Error> {
        let routed: RoutedSdkEvent = serde_json::from_value(
            notification
                .params
                .clone()
                .unwrap_or(serde_json::Value::Null),
        )?;
        let metadata = RoutedEventMetadata {
            surface_id: routed.surface_id,
            session_id: routed.envelope.session_id.clone(),
            agent_id: routed.envelope.agent_id,
            turn_id: routed.envelope.turn_id,
            session_seq: routed.envelope.session_seq,
        };
        let notifications = match routed.envelope.event.layer.as_str() {
            "protocol" => {
                let notification: ServerNotification =
                    serde_json::from_value(routed.envelope.event.payload)?;
                self.render_protocol_event(&routed.envelope.session_id, notification)
            }
            "stream" => {
                let event: AgentStreamEvent =
                    serde_json::from_value(routed.envelope.event.payload)?;
                self.render_stream_event(&routed.envelope.session_id, event)
            }
            "tui" => Vec::new(),
            other => {
                return Err(<serde_json::Error as serde::de::Error>::custom(format!(
                    "unknown routed CoreEvent layer: {other}"
                )));
            }
        };
        notifications
            .into_iter()
            .map(|notification| notification_frame(notification, &metadata))
            .collect()
    }

    fn render_protocol_event(
        &mut self,
        session_id: &SessionId,
        notification: ServerNotification,
    ) -> Vec<ServerNotification> {
        let mut rendered = Vec::new();
        match &notification {
            ServerNotification::TurnStarted(params) => {
                let mut accumulator = StreamAccumulator::new(params.turn_id.as_str().to_string());
                if let Some(buffered) = self.pre_turn.remove(session_id) {
                    rendered.extend(
                        buffered
                            .into_iter()
                            .flat_map(|event| accumulator.process(event)),
                    );
                }
                self.accumulators.insert(session_id.clone(), accumulator);
            }
            ServerNotification::TurnEnded(_) => {
                if let Some(mut accumulator) = self.accumulators.remove(session_id) {
                    rendered.extend(accumulator.flush());
                }
                self.pre_turn.remove(session_id);
            }
            _ => {}
        }
        rendered.push(notification);
        rendered
    }

    fn render_stream_event(
        &mut self,
        session_id: &SessionId,
        event: AgentStreamEvent,
    ) -> Vec<ServerNotification> {
        if let Some(accumulator) = self.accumulators.get_mut(session_id) {
            return accumulator.process(event);
        }
        let buffer = self.pre_turn.entry(session_id.clone()).or_default();
        if buffer.len() >= Self::PRE_TURN_BUFFER_CAP {
            warn!(
                session_id = %session_id,
                cap = Self::PRE_TURN_BUFFER_CAP,
                "pre-turn SDK stream buffer full; dropping event"
            );
        } else {
            buffer.push(event);
        }
        Vec::new()
    }
}

#[derive(Deserialize)]
struct RoutedSdkEvent {
    surface_id: SurfaceId,
    envelope: RoutedSdkEnvelope,
}

#[derive(Deserialize)]
struct RoutedSdkEnvelope {
    session_id: SessionId,
    #[serde(default)]
    agent_id: Option<AgentId>,
    #[serde(default)]
    turn_id: Option<TurnId>,
    #[serde(default)]
    session_seq: Option<i64>,
    event: RoutedCoreEvent,
}

#[derive(Deserialize)]
struct RoutedCoreEvent {
    layer: String,
    payload: serde_json::Value,
}

struct RoutedEventMetadata {
    surface_id: SurfaceId,
    session_id: SessionId,
    agent_id: Option<AgentId>,
    turn_id: Option<TurnId>,
    session_seq: Option<i64>,
}

fn notification_frame(
    notification: ServerNotification,
    metadata: &RoutedEventMetadata,
) -> Result<JsonRpcFrame, serde_json::Error> {
    let serde_json::Value::Object(mut value) = serde_json::to_value(notification)? else {
        unreachable!("ServerNotification always serializes as an object");
    };
    let method = match value.remove("method") {
        Some(serde_json::Value::String(method)) => method,
        _ => unreachable!("ServerNotification always serializes a string method"),
    };
    let mut params = match value.remove("params") {
        Some(serde_json::Value::Object(params)) => params,
        Some(serde_json::Value::Null) | None => serde_json::Map::new(),
        Some(other) => {
            let mut params = serde_json::Map::new();
            params.insert("payload".to_string(), other);
            params
        }
    };
    params.insert(
        "surface_id".to_string(),
        serde_json::to_value(&metadata.surface_id)?,
    );
    params.insert(
        "session_id".to_string(),
        serde_json::to_value(&metadata.session_id)?,
    );
    params.insert(
        "agent_id".to_string(),
        serde_json::to_value(&metadata.agent_id)?,
    );
    params.insert(
        "turn_id".to_string(),
        serde_json::to_value(&metadata.turn_id)?,
    );
    params.insert(
        "session_seq".to_string(),
        serde_json::to_value(metadata.session_seq)?,
    );
    Ok(JsonRpcFrame::Notification(
        TransportJsonRpcNotification::new(method, Some(serde_json::Value::Object(params))),
    ))
}

// ---------------------------------------------------------------------------
// CoreEvent → JsonRpcNotification
// ---------------------------------------------------------------------------

/// Translate a `CoreEvent` into its legacy direct-notification view.
/// Production emits canonical `session/event` envelopes; this helper remains
/// only for focused serialization tests.
///
/// See `event-system-design.md` §12.
///
/// Only used in tests — the production writer task handles dispatch inline.
#[cfg(test)]
fn core_event_to_notification(event: CoreEvent) -> Option<LegacyJsonRpcNotification> {
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
pub fn server_notification_to_jsonrpc(
    notif: ServerNotification,
) -> Option<LegacyJsonRpcNotification> {
    match serde_json::to_value(notif).ok()? {
        Value::Object(mut map) => {
            let method = match map.remove("method")? {
                Value::String(s) => s,
                _ => return None,
            };
            let params = map.remove("params").unwrap_or(Value::Null);
            Some(LegacyJsonRpcNotification {
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
