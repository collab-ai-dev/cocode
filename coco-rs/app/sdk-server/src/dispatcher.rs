//! SDK server dispatch loop.
//!
//! The `SdkServer` routes AppServer JSON-RPC frames from an SDK transport
//! into the shared AppServer host handler and writes responses plus forwarded
//! CoreEvent notifications back through the ordered writer.
//!
//! The dispatch loop reads stdin, routes control requests, and enqueues
//! messages to stdout.

use std::sync::Arc;

use coco_agent_host::app_server_host::route_app_server_session_event;
use coco_app_server::{AppServer, SessionSeqAllocator};
use coco_hub_connector::HubConnectorSender;
use coco_types::CoreEvent;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use coco_agent_host::remote_host::{
    RemoteAppServerBridgeHost, RemoteJsonRpcConnection, RemoteOutboundMessage,
};

use crate::event_renderer::SdkEventRenderer;
use crate::transport::SdkTransport;

#[cfg(test)]
use crate::event_renderer::core_event_to_notification;

/// The SDK server connection adapter.
///
/// It owns SDK transport concerns and receives shared AppServer host state
/// from startup composition; session/runtime behavior remains in `agent-host`.
///
/// Lifecycle:
/// 1. Construct with `SdkServer::new(transport, bridge_host)`.
/// 2. Create a `JsonRpcAdapterConnection`.
/// 3. Call [`Self::run_app_server_connection`]. It loops until the transport
///    closes and forwards notifications from the agent loop through the SDK
///    single-writer serializer.
pub struct SdkServer {
    transport: Arc<dyn SdkTransport>,
    bridge_host: RemoteAppServerBridgeHost,
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
}

impl SdkServer {
    /// Create a new SDK server bound to a transport.
    ///
    pub fn new(transport: Arc<dyn SdkTransport>, bridge_host: RemoteAppServerBridgeHost) -> Self {
        Self {
            transport,
            bridge_host,
            external_notifications: std::sync::Mutex::new(Vec::new()),
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

    /// Run this SDK server through the AppServer JSON-RPC adapter bridge.
    ///
    /// This preserves the SDK transport, shared AppServer host state, MCP route
    /// setup, external notification forwarding, and single-writer serializer,
    /// while delegating JSON-RPC request ownership to `coco-app-server`.
    pub async fn run_app_server_connection(
        &self,
        connection: RemoteJsonRpcConnection,
    ) -> Result<coco_app_server::DisconnectOutcome, crate::RemoteAppServerBridgeError> {
        info!("SdkServer starting AppServer bridge dispatch loop");
        let external_notifications = {
            let mut guard = self
                .external_notifications
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *guard)
        };
        crate::app_server_transport::run_app_server_connection_over_sdk_transport_with_external_notifications_and_hub_connector(
            connection,
            self.transport.clone(),
            self.bridge_host.clone(),
            external_notifications,
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
    mut outbound_rx: mpsc::Receiver<RemoteOutboundMessage>,
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
                    RemoteOutboundMessage::SessionEvent {
                        session_id,
                        event,
                        routed,
                    } => {
                        route_app_server_session_event(
                            &app_server,
                            hub_connector.as_ref(),
                            &session_seq,
                            session_id,
                            *event,
                        );
                        if let Some(routed) = routed {
                            let _ = routed.send(());
                        }
                    }
                    RemoteOutboundMessage::ProcessEvent(event) => {
                        let notification = event.into_notification();
                        if let Err(error) = transport.send_notification(&notification).await {
                            warn!(%error, "process notification forward failed");
                            break;
                        }
                    }
                    RemoteOutboundMessage::JsonRpcFrame(frame) => {
                        match renderer.render_frame(frame) {
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
                        }
                    }
                }
            }
            debug!("transport writer exited");
        },
        writer_span,
    ))
}

#[cfg(test)]
#[path = "dispatcher.test.rs"]
mod tests;
