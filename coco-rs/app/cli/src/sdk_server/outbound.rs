//! Ordered outbound SDK messages.
//!
//! The dispatcher owns a single writer task. Handlers enqueue both
//! CoreEvent notifications and JSON-RPC replies/requests here so stdout
//! observes the same order the server produced them.

use coco_types::CoreEvent;
use coco_types::SessionId;

#[derive(Debug)]
pub enum OutboundMessage {
    CoreEvent(Box<CoreEvent>),
    SessionCoreEvent {
        session_id: SessionId,
        event: Box<CoreEvent>,
    },
    JsonRpcFrame(coco_app_server_transport::JsonRpcFrame),
}

impl OutboundMessage {
    pub fn core_event(event: CoreEvent) -> Self {
        Self::CoreEvent(Box::new(event))
    }

    pub fn session_core_event(session_id: SessionId, event: CoreEvent) -> Self {
        Self::SessionCoreEvent {
            session_id,
            event: Box::new(event),
        }
    }
}
