//! coco-rs SDK control protocol — strict JSON-RPC 2.0 wire envelope.
//!
//! SDK clients send one JSON-RPC message per NDJSON line. Requests and
//! responses correlate through `id`; notifications omit `id`; errors use
//! the standard nested `{ code, message, data? }` object. Batch requests
//! are intentionally unsupported.
//!
//! See `event-system-design.md` §1.4.

use serde::Deserialize;
use serde::Serialize;

pub const JSONRPC_VERSION: &str = "2.0";

fn deserialize_jsonrpc_version<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value == JSONRPC_VERSION {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(format!(
            "invalid JSON-RPC version: expected {JSONRPC_VERSION}, got {value}"
        )))
    }
}

/// Request identifier. Can be a string or integer per JSON-RPC 2.0.
/// SDK clients typically use integers; coco-rs accepts both.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Integer(i64),
    String(String),
}

impl RequestId {
    /// Convert to a display string for logging.
    pub fn as_display(&self) -> String {
        match self {
            Self::Integer(i) => i.to_string(),
            Self::String(s) => s.clone(),
        }
    }
}

/// Top-level JSON-RPC 2.0 message.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    /// Request expecting a response. Correlates via `id`.
    Request(JsonRpcRequest),
    /// Response to a previously-sent request.
    Response(JsonRpcResponse),
    /// Fire-and-forget notification (no response expected).
    /// `ServerNotification` is the usual payload in coco-rs.
    Notification(JsonRpcNotification),
    /// Error reply (alternative to `Response` for failures).
    Error(JsonRpcError),
}

/// A JSON-RPC request wrapper. Holds the method name + params.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    #[serde(deserialize_with = "deserialize_jsonrpc_version")]
    pub jsonrpc: String,
    /// Unique identifier for correlating the response.
    #[serde(rename = "id")]
    pub request_id: RequestId,
    /// Dispatch key (e.g. "turn/start", "mcp/status").
    pub method: String,
    /// Method-specific parameters.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Successful response payload.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    #[serde(deserialize_with = "deserialize_jsonrpc_version")]
    pub jsonrpc: String,
    #[serde(rename = "id")]
    pub request_id: RequestId,
    /// Method-specific result value.
    #[serde(default)]
    pub result: serde_json::Value,
}

/// Error response payload. Mirrors JSON-RPC 2.0 error structure.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    #[serde(deserialize_with = "deserialize_jsonrpc_version")]
    pub jsonrpc: String,
    #[serde(rename = "id")]
    pub request_id: RequestId,
    pub error: JsonRpcErrorObject,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Fire-and-forget notification. In coco-rs this is the primary outbound
/// format for `ServerNotification` events (no `request_id`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    #[serde(deserialize_with = "deserialize_jsonrpc_version")]
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Standard JSON-RPC 2.0 error codes plus local extensions.
pub mod error_codes {
    /// Malformed JSON received.
    pub const PARSE_ERROR: i32 = -32700;
    /// Request does not conform to JSON-RPC 2.0 shape.
    pub const INVALID_REQUEST: i32 = -32600;
    /// Method name not recognized by the server.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Method params failed schema validation.
    pub const INVALID_PARAMS: i32 = -32602;
    /// Internal server error.
    pub const INTERNAL_ERROR: i32 = -32603;

    // Local extensions (>= -32000 per JSON-RPC reserved range)
    /// Request cancelled by the server (e.g. turn/interrupt).
    pub const REQUEST_CANCELLED: i32 = -32001;
    /// Permission denied for the requested action.
    pub const PERMISSION_DENIED: i32 = -32002;
    /// Session not initialized; send `initialize` first.
    pub const NOT_INITIALIZED: i32 = -32003;
}

#[cfg(test)]
#[path = "jsonrpc.test.rs"]
mod tests;
