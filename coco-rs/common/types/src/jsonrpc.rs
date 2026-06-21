//! coco-rs SDK control protocol — wire envelope.
//!
//! **Not strict JSON-RPC 2.0** — there is no `jsonrpc: "2.0"` field
//! on the wire. The shape is JSON-RPC-*like* (request_id / method /
//! params / result), promoted from the TS SDK's `control_request` /
//! `control_response` discriminated union for clearer correlation and
//! cross-language codegen.
//!
//! **Not strictly TS-mirror** — TS uses
//! `{type: "control_request", request_id, request: {subtype, ...}}` /
//! `{type: "control_response", response: {subtype, request_id, response?, error?}}`
//! with `subtype` as the method discriminator nested under
//! `request`/`response`. coco-rs flattens that to
//! `{type: "request"|"response"|"notification"|"error", request_id,
//! method, params|result|error}` so the wire is one level shallower
//! and JSON-RPC tooling Just Works. Functionally equivalent contract;
//! different envelope tags. SDK clients targeting coco-rs follow this
//! shape, not the TS one — see `coco-sdk/python/_message_router.py`.
//!
//! See `event-system-design.md` §1.4.

use serde::Deserialize;
use serde::Serialize;

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

/// Top-level wire message. SDK clients send these over stdin; coco-rs
/// writes these to stdout. Consumers dispatch on the `type` discriminator
/// (NOT `method` — JSON-RPC's `method` field is inside the inner payload).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JsonRpcMessage {
    /// Request expecting a response. Correlates via `request_id`.
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
    /// Unique identifier for correlating the response.
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
    pub request_id: RequestId,
    /// Method-specific result value.
    #[serde(default)]
    pub result: serde_json::Value,
}

/// Error response payload. Mirrors JSON-RPC 2.0 error structure.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub request_id: RequestId,
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
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Standard JSON-RPC 2.0 error codes plus coco-rs extensions.
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

    // coco-rs extensions (≥ -32000 per JSON-RPC reserved range)
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
