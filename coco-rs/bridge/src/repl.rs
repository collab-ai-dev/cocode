//! REPL bridge for headless/non-TUI communication.
//!
//! Provides input/output message routing and session state
//! synchronization for SDK and daemon callers that bypass the TUI.
//! The REPL bridge sits between the agent loop and a transport
//! (WebSocket, SSE, or NDJSON) and handles bidirectional message
//! serialization, keepalive, and reconnection awareness.
//!
//! Control requests (`ReplInMessage::ControlRequest`) are dispatched
//! through the [`ControlRequestHandler`] trait. Bridge consumers wire
//! a concrete handler at session startup; the bridge supplies the
//! [`dispatch_control`] helper so the ReplOutMessage shaping stays in
//! one place. The default [`RejectingControlHandler`] fails closed so
//! an un-wired bridge never silently accepts a privileged request.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;

use coco_types::PermissionMode;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tracing::info;
use tracing::warn;

// ---------------------------------------------------------------------------
// REPL Bridge Messages
// ---------------------------------------------------------------------------

/// Inbound message from an SDK consumer or remote client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReplInMessage {
    /// User prompt submission.
    UserMessage { text: String },
    /// Control request from SDK (initialize, interrupt, permission, etc.).
    ControlRequest {
        request_id: String,
        #[serde(flatten)]
        request: ControlRequest,
    },
    /// Cancel a previously submitted async message.
    CancelAsyncMessage { message_uuid: String },
    /// Permission response (approve/deny a pending tool use).
    PermissionResponse {
        request_id: String,
        #[serde(flatten)]
        decision: PermissionDecision,
    },
    /// Keepalive ping.
    Ping,
}

/// Outbound message to an SDK consumer or remote client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReplOutMessage {
    /// Assistant text chunk (streaming).
    StreamEvent { content: String },
    /// Complete assistant message.
    AssistantMessage { text: String },
    /// Tool execution started.
    ToolUseStart {
        tool_use_id: String,
        tool_name: String,
    },
    /// Tool execution completed.
    ToolUseEnd {
        tool_use_id: String,
        tool_name: String,
        is_error: bool,
    },
    /// Control request from agent to SDK (permission, input, etc.).
    ControlRequest {
        request_id: String,
        #[serde(flatten)]
        request: SdkControlOutbound,
    },
    /// Control response to an SDK control request.
    ControlResponse {
        request_id: String,
        #[serde(flatten)]
        response: serde_json::Value,
    },
    /// Session result (final output).
    Result {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    /// Error message.
    Error { message: String },
    /// Keepalive pong.
    Pong,
}

/// SDK control requests (subset relevant to REPL bridge).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum ControlRequest {
    /// Initialize SDK session.
    Initialize {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_prompt: Option<String>,
    },
    /// Interrupt current turn.
    Interrupt,
    /// Set model for subsequent turns.
    SetModel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Set permission mode.
    ///
    /// Runtime dispatch goes through [`ControlRequestHandler::handle`].
    /// The production handler (`app/cli`) must reject
    /// `BypassPermissions` when the session's startup capability gate
    /// is off. The default [`RejectingControlHandler`] fails closed so
    /// an un-wired bridge never silently grants bypass.
    SetPermissionMode { mode: PermissionMode },
    /// Query MCP server status.
    McpStatus,
    /// Get context window usage.
    GetContextUsage,
    /// Rewind files to a previous message.
    RewindFiles {
        user_message_id: String,
        #[serde(default)]
        dry_run: bool,
    },
}

/// Permission decision from SDK consumer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Deny,
    AllowAlways { scope: String },
}

/// Outbound control requests from agent to SDK.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum SdkControlOutbound {
    /// Request permission to use a tool.
    CanUseTool {
        tool_name: String,
        tool_use_id: String,
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// Request user input.
    RequestInput {
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// REPL Bridge State
// ---------------------------------------------------------------------------

/// Connection state of the REPL bridge transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum BridgeState {
    /// Bridge created but not yet connected.
    Idle = 0,
    /// Transport connected and active.
    Connected = 1,
    /// Transport disconnected, attempting reconnection.
    Reconnecting = 2,
    /// Permanently failed (auth rejected, session expired).
    Failed = 3,
}

impl BridgeState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Connected,
            2 => Self::Reconnecting,
            3 => Self::Failed,
            _ => Self::Idle,
        }
    }
}

// ---------------------------------------------------------------------------
// REPL Bridge
// ---------------------------------------------------------------------------

/// Maximum number of messages to buffer when transport is disconnected.
const MAX_OUTBOUND_BUFFER: usize = 1000;

/// REPL bridge for headless/non-TUI communication.
///
/// Routes messages between the agent loop and a transport layer
/// (SSE, WebSocket, NDJSON). Buffers outbound messages during
/// disconnection and drains on reconnect.
pub struct ReplBridge {
    /// Session identifier.
    session_id: String,
    /// Current bridge state (atomic for lock-free reads in `send()`).
    state: AtomicU8,
    /// Watch channel for state change notifications.
    state_notify: watch::Sender<BridgeState>,
    /// Incoming messages from SDK/remote.
    incoming_tx: mpsc::Sender<ReplInMessage>,
    incoming_rx: Option<mpsc::Receiver<ReplInMessage>>,
    /// Outgoing messages to SDK/remote.
    outgoing_tx: mpsc::Sender<ReplOutMessage>,
    outgoing_rx: Option<mpsc::Receiver<ReplOutMessage>>,
    /// Buffered outbound messages during disconnection.
    buffer: Arc<Mutex<VecDeque<ReplOutMessage>>>,
}

impl ReplBridge {
    /// Create a new REPL bridge for the given session.
    pub fn new(session_id: String) -> Self {
        let (incoming_tx, incoming_rx) = mpsc::channel(256);
        let (outgoing_tx, outgoing_rx) = mpsc::channel(256);
        let (state_notify, _) = watch::channel(BridgeState::Idle);

        Self {
            session_id,
            state: AtomicU8::new(BridgeState::Idle as u8),
            state_notify,
            incoming_tx,
            incoming_rx: Some(incoming_rx),
            outgoing_tx,
            outgoing_rx: Some(outgoing_rx),
            buffer: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Get the session identifier.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Get the current bridge state.
    pub fn state(&self) -> BridgeState {
        BridgeState::from_u8(self.state.load(Ordering::SeqCst))
    }

    /// Subscribe to state changes.
    pub fn watch_state(&self) -> watch::Receiver<BridgeState> {
        self.state_notify.subscribe()
    }

    /// Transition to a new state.
    pub fn set_state(&self, new_state: BridgeState) {
        let old = self.state();
        if old != new_state {
            info!(
                session_id = %self.session_id,
                old = ?old,
                new = ?new_state,
                "REPL bridge state transition"
            );
            self.state.store(new_state as u8, Ordering::SeqCst);
            // Ignore send error -- no receivers is fine.
            let _ = self.state_notify.send(new_state);
        }
    }

    /// Take the incoming message receiver (agent loop consumes this).
    pub fn take_incoming(&mut self) -> Option<mpsc::Receiver<ReplInMessage>> {
        self.incoming_rx.take()
    }

    /// Take the outgoing message receiver (transport consumes this).
    pub fn take_outgoing(&mut self) -> Option<mpsc::Receiver<ReplOutMessage>> {
        self.outgoing_rx.take()
    }

    /// Get a sender for injecting incoming messages.
    pub fn incoming_sender(&self) -> mpsc::Sender<ReplInMessage> {
        self.incoming_tx.clone()
    }

    /// Send a message to the SDK/remote client.
    ///
    /// If the transport is disconnected, buffers the message up to
    /// `MAX_OUTBOUND_BUFFER`. Returns `Ok(())` even when buffered.
    pub async fn send(&self, msg: ReplOutMessage) -> crate::Result<()> {
        if self.state() == BridgeState::Connected {
            self.outgoing_tx
                .send(msg)
                .await
                .map_err(|_| crate::BridgeError::ChannelClosed)?;
        } else {
            let mut buf = self.buffer.lock().await;
            if buf.len() >= MAX_OUTBOUND_BUFFER {
                warn!(
                    session_id = %self.session_id,
                    buffer_size = buf.len(),
                    "REPL bridge outbound buffer full, dropping oldest"
                );
                buf.pop_front();
            }
            buf.push_back(msg);
        }
        Ok(())
    }

    /// Drain buffered messages after reconnection.
    pub async fn drain_buffer(&self) -> crate::Result<()> {
        let messages: Vec<ReplOutMessage> = {
            let mut buf = self.buffer.lock().await;
            buf.drain(..).collect()
        };

        let count = messages.len();
        for msg in messages {
            self.outgoing_tx
                .send(msg)
                .await
                .map_err(|_| crate::BridgeError::ChannelClosed)?;
        }

        if count > 0 {
            info!(
                session_id = %self.session_id,
                drained = count,
                "REPL bridge drained buffered messages"
            );
        }

        Ok(())
    }

    /// Send a result message and transition to idle.
    pub async fn send_result(&self, text: String) -> crate::Result<()> {
        self.send(ReplOutMessage::Result {
            text,
            session_id: Some(self.session_id.clone()),
        })
        .await
    }

    /// Send an error message.
    pub async fn send_error(&self, message: String) -> crate::Result<()> {
        self.send(ReplOutMessage::Error { message }).await
    }

    /// Number of messages currently buffered.
    pub async fn buffer_len(&self) -> usize {
        self.buffer.lock().await.len()
    }
}

/// Decode an NDJSON line into a REPL inbound message.
pub fn decode_repl_ndjson(line: &str) -> crate::Result<ReplInMessage> {
    let msg: ReplInMessage = serde_json::from_str(line.trim())?;
    Ok(msg)
}

/// Encode a REPL outbound message as NDJSON.
pub fn encode_repl_ndjson(msg: &ReplOutMessage) -> crate::Result<String> {
    let json = serde_json::to_string(msg)?;
    Ok(format!("{json}\n"))
}

// ---------------------------------------------------------------------------
// Control request dispatch
// ---------------------------------------------------------------------------

/// Error returned from a control handler. The `code` is a JSON-RPC
/// application error code (negative integer in the `-32000..=-32099`
/// range conventionally, but any i32 works on the wire); the
/// `message` is surfaced back to the SDK client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlError {
    pub code: i32,
    pub message: String,
}

impl ControlError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Dispatcher for inbound [`ControlRequest`] payloads.
///
/// Implementors hold whatever session state the specific control
/// request needs (permission gate, model registry, session handle,
/// etc.) and translate the request into a side-effect + response
/// payload. The bridge itself is transport-only and does NOT embed
/// any policy — policy lives in the handler.
///
/// **Security contract for `SetPermissionMode`:** the handler MUST
/// check the session's startup bypass capability before accepting a
/// transition into `PermissionMode::BypassPermissions`. Reject with
/// [`ControlError`] carrying
/// `coco_types::error_codes::PERMISSION_DENIED` when the gate is off.
/// This matches the SDK `handle_set_permission_mode` and TUI
/// `UserCommand::SetPermissionMode` guards — all three bypass-origin
/// sites enforce the same rule.
#[async_trait::async_trait]
pub trait ControlRequestHandler: Send + Sync {
    async fn handle(&self, request: ControlRequest) -> Result<serde_json::Value, ControlError>;
}

/// Fail-closed default handler: every control request returns
/// `METHOD_NOT_FOUND`.
///
/// Use this when wiring the bridge in a context that doesn't yet own
/// session state. It's the safe baseline — an un-wired bridge never
/// silently accepts a privileged request like bypass escalation.
/// Replace with a real handler once the session layer is available.
pub struct RejectingControlHandler;

#[async_trait::async_trait]
impl ControlRequestHandler for RejectingControlHandler {
    async fn handle(&self, request: ControlRequest) -> Result<serde_json::Value, ControlError> {
        Err(ControlError::new(
            coco_types::error_codes::METHOD_NOT_FOUND,
            format!("bridge control handler not wired; request {request:?} rejected"),
        ))
    }
}

/// Dispatch a single control request through the handler and shape
/// the result into the appropriate [`ReplOutMessage`].
///
/// Success → `ControlResponse { request_id, response }`.
/// Failure → `ControlResponse` with an `{ error: { code, message } }`
/// payload (JSON-RPC-style error within the result envelope). The
/// bridge-level `Error { message }` variant is reserved for
/// transport-level faults, not application errors; keeping the
/// request_id on the response lets the client correlate.
pub async fn dispatch_control<H: ControlRequestHandler + ?Sized>(
    handler: &H,
    request_id: String,
    request: ControlRequest,
) -> ReplOutMessage {
    match handler.handle(request).await {
        Ok(value) => ReplOutMessage::ControlResponse {
            request_id,
            response: value,
        },
        Err(err) => ReplOutMessage::ControlResponse {
            request_id,
            response: serde_json::json!({
                "error": {
                    "code": err.code,
                    "message": err.message,
                }
            }),
        },
    }
}

#[cfg(test)]
#[path = "repl.test.rs"]
mod tests;
