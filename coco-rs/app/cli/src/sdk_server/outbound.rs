//! Ordered outbound SDK messages.
//!
//! The dispatcher owns a single writer task. Handlers enqueue both
//! CoreEvent notifications and JSON-RPC replies/requests here so stdout
//! observes the same order the server produced them.

use coco_types::CoreEvent;
use coco_types::JsonRpcMessage;

#[derive(Debug)]
pub enum OutboundMessage {
    CoreEvent(Box<CoreEvent>),
    JsonRpc(JsonRpcMessage),
}

impl OutboundMessage {
    pub fn core_event(event: CoreEvent) -> Self {
        Self::CoreEvent(Box::new(event))
    }
}
