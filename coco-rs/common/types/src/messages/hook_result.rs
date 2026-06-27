use serde::Deserialize;
use serde::Serialize;

use crate::HookOutcome;
use crate::PermissionBehavior;

use super::Message;

/// Result returned from a hook execution.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    pub outcome: HookOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_behavior: Option<PermissionBehavior>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<serde_json::Value>,
    /// Human-readable status for progress display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
    /// When true, the hook runner should re-wake after async completion.
    #[serde(default)]
    pub async_rewake: bool,
}
