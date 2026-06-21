//! SDK-compatible query types.
//!
//! Types for the SDK query interface.

use serde::Deserialize;
use serde::Serialize;

/// SDK query options.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SdkQueryOptions {
    pub model: Option<String>,
    pub max_tokens: Option<i64>,
    pub max_turns: Option<i32>,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub permission_mode: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub include_hook_events: bool,
    pub cwd: Option<String>,
}

/// SDK message item (for structured output).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SdkItem {
    /// Agent's text response.
    AgentMessage {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Tool execution.
    ToolUse {
        tool_name: String,
        tool_use_id: String,
        input: serde_json::Value,
        output: Option<String>,
        is_error: bool,
        duration_ms: i64,
    },
    /// File change.
    FileChange {
        path: String,
        change_type: FileChangeType,
    },
    /// Reasoning/thinking block.
    Reasoning { text: String },
    /// Error.
    Error {
        message: String,
        code: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeType {
    Create,
    Update,
    Delete,
}

/// SDK turn result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkTurnResult {
    pub items: Vec<SdkItem>,
    pub turn_number: i32,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub stop_reason: Option<String>,
}

/// SDK session result (complete query result).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkSessionResult {
    pub turns: Vec<SdkTurnResult>,
    pub total_turns: i32,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cost_usd: f64,
    pub session_id: String,
    pub model: String,
}

#[cfg(test)]
#[path = "sdk_types.test.rs"]
mod tests;
