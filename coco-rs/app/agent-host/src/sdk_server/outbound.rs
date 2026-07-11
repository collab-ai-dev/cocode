//! Ordered outbound SDK messages.
//!
//! The dispatcher owns a single writer task. Handlers enqueue both
//! CoreEvent notifications and JSON-RPC replies/requests here so stdout
//! observes the same order the server produced them.

use coco_types::AgentId;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::SessionId;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

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

pub async fn send_session_event(
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

pub async fn send_session_event_and_wait(
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
pub enum SessionEventSendError {
    #[error("session event router is closed")]
    Closed,
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
