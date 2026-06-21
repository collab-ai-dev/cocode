use serde::Deserialize;
use serde::Serialize;

use crate::ToolId;

/// Stream events emitted during API response processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    TextDelta { text: String },
    ThinkingDelta { text: String },
    ToolUseStart { id: String, tool_id: ToolId },
    ToolUseInput { id: String, delta: String },
    ToolUseEnd { id: String },
    RequestStart(RequestStartEvent),
    MessageComplete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestStartEvent {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Task budget for API output pacing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskBudget {
    pub total: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<i64>,
}

/// Streaming tool use accumulation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingToolUse {
    pub id: String,
    pub tool_id: ToolId,
    /// Accumulated JSON string.
    pub input_json: String,
}

/// Streaming thinking accumulation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamingThinking {
    pub text: String,
}
