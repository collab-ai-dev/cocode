//! IDE-side permission relay.
//!
//! When the agent needs approval for a tool call and an IDE is
//! connected, the bridge forwards the approval request to the IDE's
//! native UI instead of showing the TUI's own permission overlay.
//!
//! The actual wire protocol is implemented by `BridgeServer` (see
//! `server.rs`). This module provides the pure-logic shape of the
//! request/response pair so both sides of the bridge can depend on the
//! same types.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// Request sent from agent → IDE when a tool needs approval.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgePermissionRequest {
    /// Unique correlation ID for matching the response back.
    pub id: String,
    /// Tool name (e.g. `"Bash"`, `"Write"`).
    pub tool_name: String,
    /// Human-readable description of what the tool will do.
    pub description: String,
    /// Tool-call id the IDE should associate with any rendering.
    pub tool_use_id: String,
    /// Tool input in JSON form for the IDE to render.
    pub input: Value,
    /// Optional risk badge to inform the IDE's styling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<BridgeRiskLevel>,
    /// Whether the IDE should offer an "always allow" toggle.
    #[serde(default)]
    pub show_always_allow: bool,
}

/// Risk classification fed to the IDE so it can style / highlight the
/// approval UI. Lives here to keep the bridge schema self-contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BridgeRiskLevel {
    Low,
    Medium,
    High,
}

/// Response posted from IDE → agent once the user decides.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgePermissionResponse {
    /// Correlation id echoing the originating request.
    pub id: String,
    /// Accept / reject decision.
    pub decision: BridgeDecision,
    /// Optional reason (the IDE may collect one on reject; "always
    /// allow" selections carry a reason too for audit purposes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Whether the user asked for a persistent always-allow rule.
    #[serde(default)]
    pub always_allow: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BridgeDecision {
    Approved,
    Rejected,
}

#[cfg(test)]
#[path = "permission_callbacks.test.rs"]
mod tests;
