//! High-level content parts from generate_text / stream_text results.
//!
//! These types represent the user-facing content parts returned in step results,
//! distinct from the provider-level `AssistantContentPart`.

use super::generated_file::GeneratedFile;
use super::reasoning_output::ReasoningOutput;

/// A high-level content part from text generation.
///
/// This enum represents the various content parts returned from
/// `generate_text` / `stream_text`, including text, reasoning,
/// sources, files, tool calls, tool results, and tool approval requests.
#[derive(Debug, Clone)]
pub enum ContentPart {
    /// Text content.
    Text {
        /// The text.
        text: String,
    },
    /// Reasoning / chain-of-thought content.
    Reasoning(ReasoningOutput),
    /// A source reference.
    Source(vercel_ai_provider::Source),
    /// A generated file.
    File(GeneratedFile),
    /// A tool call made by the model.
    ToolCall {
        /// The tool call ID.
        tool_call_id: String,
        /// The tool name.
        tool_name: String,
        /// The tool arguments.
        args: serde_json::Value,
    },
    /// A tool result.
    ToolResult {
        /// The tool call ID this result corresponds to.
        tool_call_id: String,
        /// The tool name.
        tool_name: String,
        /// The result content.
        result: serde_json::Value,
    },
    /// A tool error.
    ToolError {
        /// The tool call ID.
        tool_call_id: String,
        /// The tool name.
        tool_name: String,
        /// The error message.
        error: String,
    },
    /// A tool approval request (when tool execution requires user approval).
    ToolApprovalRequest {
        /// The tool call ID.
        tool_call_id: String,
        /// The tool name.
        tool_name: String,
        /// The tool arguments.
        args: serde_json::Value,
    },
}

impl ContentPart {
    /// Create a text content part.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Create a tool call content part.
    pub fn tool_call(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
        Self::ToolCall {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            args,
        }
    }

    /// Create a tool result content part.
    pub fn tool_result(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        result: serde_json::Value,
    ) -> Self {
        Self::ToolResult {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            result,
        }
    }
}

/// Represents a denied tool execution.
///
/// When a tool call requires user approval and is denied, this type
/// captures the details of the denial.
#[derive(Debug, Clone)]
pub struct ToolOutputDenied {
    /// The tool call ID that was denied.
    pub tool_call_id: String,
    /// The tool name that was denied.
    pub tool_name: String,
    /// The reason for the denial.
    pub reason: Option<String>,
}

impl ToolOutputDenied {
    /// Create a new tool output denied.
    pub fn new(tool_call_id: impl Into<String>, tool_name: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            reason: None,
        }
    }

    /// Set the denial reason.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}
