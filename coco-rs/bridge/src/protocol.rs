//! Bridge protocol types.

use serde::Deserialize;
use serde::Serialize;

/// Bridge transport protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeTransport {
    WebSocket,
    Sse,
    Ndjson,
}

/// Message from IDE to agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeInMessage {
    /// Submit user input text.
    Submit { text: String },
    /// Approve a pending tool use.
    Approve { tool_use_id: String },
    /// Deny a pending tool use.
    Deny { tool_use_id: String },
    /// Cancel current operation.
    Cancel,
    /// Keepalive ping.
    Ping,
}

/// Message from agent to IDE.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeOutMessage {
    /// Text output from agent.
    Text { content: String },
    /// Tool is being used.
    ToolUse {
        tool_name: String,
        tool_use_id: String,
        input: serde_json::Value,
    },
    /// Tool execution result.
    ToolResult { tool_use_id: String, output: String },
    /// Permission required for a tool use.
    PermissionRequest {
        tool_use_id: String,
        message: String,
    },
    /// Status update.
    Status { model: String, session_id: String },
    /// Error message.
    Error { message: String },
    /// Keepalive pong.
    Pong,
}

/// Encode a message as NDJSON (newline-delimited JSON).
pub fn encode_ndjson(msg: &BridgeOutMessage) -> crate::Result<String> {
    let json = serde_json::to_string(msg)?;
    Ok(format!("{json}\n"))
}

/// Decode an incoming NDJSON message.
pub fn decode_ndjson(line: &str) -> crate::Result<BridgeInMessage> {
    let msg: BridgeInMessage = serde_json::from_str(line.trim())?;
    Ok(msg)
}
