//! Bridge server — accepts IDE connections and routes messages.

use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::mpsc;

use crate::protocol::BridgeInMessage;
use crate::protocol::BridgeOutMessage;

/// Bridge server managing IDE connections.
pub struct BridgeServer {
    /// Channel for incoming messages from IDE.
    incoming_tx: mpsc::Sender<BridgeInMessage>,
    incoming_rx: Option<mpsc::Receiver<BridgeInMessage>>,
    /// Broadcast channel for outgoing messages to IDE.
    outgoing_tx: broadcast::Sender<BridgeOutMessage>,
    /// Whether the server is running.
    running: bool,
}

impl BridgeServer {
    /// Create a new bridge server.
    pub fn new() -> Self {
        let (incoming_tx, incoming_rx) = mpsc::channel(256);
        let (outgoing_tx, _) = broadcast::channel(256);
        Self {
            incoming_tx,
            incoming_rx: Some(incoming_rx),
            outgoing_tx,
            running: false,
        }
    }

    /// Get sender for injecting incoming messages (for testing or NDJSON transport).
    pub fn incoming_sender(&self) -> mpsc::Sender<BridgeInMessage> {
        self.incoming_tx.clone()
    }

    /// Take the incoming message receiver (can only be called once).
    pub fn take_incoming_receiver(&mut self) -> Option<mpsc::Receiver<BridgeInMessage>> {
        self.incoming_rx.take()
    }

    /// Subscribe to outgoing messages (each subscriber gets all messages).
    pub fn subscribe_outgoing(&self) -> broadcast::Receiver<BridgeOutMessage> {
        self.outgoing_tx.subscribe()
    }

    /// Send a message to all connected IDEs.
    pub fn send(&self, msg: BridgeOutMessage) -> crate::Result<()> {
        self.outgoing_tx
            .send(msg)
            .map_err(|_| crate::BridgeError::NoSubscribers)?;
        Ok(())
    }

    /// Send text output.
    pub fn send_text(&self, content: &str) -> crate::Result<()> {
        self.send(BridgeOutMessage::Text {
            content: content.to_string(),
        })
    }

    /// Send error message.
    pub fn send_error(&self, message: &str) -> crate::Result<()> {
        self.send(BridgeOutMessage::Error {
            message: message.to_string(),
        })
    }

    /// Check if server is running.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Shared reference for passing across tasks.
    pub fn into_shared(self) -> Arc<Self> {
        Arc::new(self)
    }
}

impl Default for BridgeServer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "server.test.rs"]
mod tests;
