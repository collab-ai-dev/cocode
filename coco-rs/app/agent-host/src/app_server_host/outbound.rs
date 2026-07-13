use std::sync::Arc;

use coco_app_server::AppServer;
use coco_hub_connector::HubConnectorSender;
use coco_types::{AgentId, CoreEvent, ServerNotification, SessionEnvelope, SessionId};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tracing::warn;

use super::AppServerHostState;

#[derive(Debug)]
pub enum OutboundMessage {
    SessionEvent {
        session_id: SessionId,
        event: Box<CoreEvent>,
        routed: Option<oneshot::Sender<()>>,
    },
    ProcessEvent(ProcessEvent),
    JsonRpcFrame(coco_app_server_transport::JsonRpcFrame),
}

#[derive(Debug)]
pub enum ProcessEvent {
    PluginsChanged { reason: String },
}

impl ProcessEvent {
    pub fn from_core_event(event: CoreEvent) -> Option<Self> {
        match event {
            CoreEvent::Protocol(ServerNotification::PluginsChanged { reason }) => {
                Some(Self::PluginsChanged { reason })
            }
            CoreEvent::Protocol(_) | CoreEvent::Stream(_) | CoreEvent::Tui(_) => None,
        }
    }

    pub fn into_notification(self) -> ServerNotification {
        match self {
            Self::PluginsChanged { reason } => ServerNotification::PluginsChanged { reason },
        }
    }
}

pub(crate) async fn send_session_event(
    tx: &mpsc::Sender<OutboundMessage>,
    session_id: SessionId,
    event: CoreEvent,
) -> Result<(), SessionEventSendError> {
    tx.send(OutboundMessage::SessionEvent {
        session_id,
        event: Box::new(event),
        routed: None,
    })
    .await
    .map_err(|_| SessionEventSendError::Closed)
}

pub(crate) async fn send_session_event_and_wait(
    tx: &mpsc::Sender<OutboundMessage>,
    session_id: SessionId,
    event: CoreEvent,
) -> Result<(), SessionEventSendError> {
    let (routed, routed_rx) = oneshot::channel();
    tx.send(OutboundMessage::SessionEvent {
        session_id,
        event: Box::new(event),
        routed: Some(routed),
    })
    .await
    .map_err(|_| SessionEventSendError::Closed)?;
    routed_rx.await.map_err(|_| SessionEventSendError::Closed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SessionEventSendError {
    #[error("session event router is closed")]
    Closed,
}

pub fn spawn_app_server_local_outbound_forwarder<H>(
    server: Arc<AppServer<H>>,
    state: Arc<AppServerHostState>,
    mut outbound_rx: mpsc::Receiver<OutboundMessage>,
    hub_connector: Arc<std::sync::RwLock<Option<HubConnectorSender>>>,
) -> JoinHandle<()>
where
    H: Clone + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let session_seq = Arc::clone(state.session_seq_allocator());
        while let Some(outbound) = outbound_rx.recv().await {
            match outbound {
                OutboundMessage::SessionEvent {
                    session_id,
                    event,
                    routed,
                } => {
                    let hub_connector = clone_hub_connector_sender(&hub_connector);
                    route_app_server_session_event(
                        &server,
                        hub_connector.as_ref(),
                        &session_seq,
                        session_id,
                        *event,
                    );
                    if let Some(routed) = routed {
                        let _ = routed.send(());
                    }
                }
                OutboundMessage::ProcessEvent(_) => {
                    warn!("dropping process event on local AppServer forwarder");
                }
                OutboundMessage::JsonRpcFrame(_) => {
                    warn!("dropping JSON-RPC outbound message on local AppServer forwarder");
                }
            }
        }
    })
}

/// Configure the process-shared durable `session_seq` allocator:
/// bind the skip-ahead window to the retention ring size and install a
/// best-effort persist hook that appends each due watermark to the session's
/// transcript. Idempotent — repeated setup only re-binds the window and hook.
pub fn install_session_seq_durability(state: &Arc<AppServerHostState>, event_retention: i64) {
    let allocator = state.session_seq_allocator();
    allocator.set_skip_ahead_window(event_retention);
    // Weak reference so the hook never keeps `AppServerHostState` (which owns the
    // allocator) alive — otherwise state -> allocator -> hook -> state leaks.
    let weak_state = Arc::downgrade(state);
    allocator.set_persist_hook(Arc::new(move |session_id, session_seq| {
        let Some(state) = weak_state.upgrade() else {
            return;
        };
        let session_id = session_id.clone();
        // The hook fires from inside the forwarder task (a Tokio context), so
        // resolving the manager and writing the transcript can be deferred off
        // the routing path.
        tokio::spawn(async move {
            let Some(manager) = state.session_manager_snapshot().await else {
                return;
            };
            let id = session_id.as_str().to_string();
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(error) = manager.persist_session_seq_watermark(&id, session_seq) {
                    tracing::debug!(%error, "failed to persist session_seq watermark");
                }
            })
            .await;
        });
    }));
}

fn clone_hub_connector_sender(
    hub_connector: &Arc<std::sync::RwLock<Option<HubConnectorSender>>>,
) -> Option<HubConnectorSender> {
    match hub_connector.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

pub fn route_app_server_session_event<H>(
    server: &AppServer<H>,
    hub_connector: Option<&HubConnectorSender>,
    session_seq: &coco_app_server::SessionSeqAllocator,
    session_id: SessionId,
    event: CoreEvent,
) where
    H: Clone + Send + Sync + 'static,
{
    let seq_session_id = session_id.clone();
    let agent_id = event_agent_id(&event);
    let envelope = SessionEnvelope::stamp(session_id, agent_id, event, || {
        session_seq.next(&seq_session_id)
    });
    let hub_envelope = envelope.clone();
    server.route_envelope(envelope);
    if let Some(hub_connector) = hub_connector
        && let Err(error) = hub_connector.try_enqueue(hub_envelope)
    {
        warn!(%error, "dropping AppServer event from Hub connector queue");
    }
}

pub fn event_agent_id(event: &CoreEvent) -> Option<AgentId> {
    let CoreEvent::Protocol(notification) = event else {
        return None;
    };
    let raw = notification.agent_id()?;
    match AgentId::try_new(raw) {
        Ok(agent_id) => Some(agent_id),
        Err(error) => {
            tracing::warn!(%error, agent_id = raw, "ignoring invalid event agent id");
            None
        }
    }
}
