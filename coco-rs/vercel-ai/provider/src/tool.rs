//! Tool invocation type.
//!
//! Tool definition types live in
//! [`crate::language_model::v4::function_tool::LanguageModelV4FunctionTool`]
//! (alias `ToolDefinitionV4`). Tool call / tool result types live in
//! [`crate::language_model::v4::tool_call::LanguageModelV4ToolCall`] /
//! [`crate::language_model::v4::tool_result::LanguageModelV4ToolResult`].

use serde::Deserialize;
use serde::Serialize;

use crate::json_value::JSONValue;

/// A tool invocation (combination of tool call and optional result).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInvocation {
    /// The tool call.
    pub tool_call_id: String,
    /// The tool name.
    pub tool_name: String,
    /// The tool arguments.
    pub input: JSONValue,
    /// The tool result (if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<JSONValue>,
    /// Whether the result is an error.
    #[serde(default)]
    pub is_error: bool,
}

impl ToolInvocation {
    /// Create a new tool invocation without a result.
    pub fn pending(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        input: JSONValue,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            input,
            output: None,
            is_error: false,
        }
    }

    /// Add a result to the invocation.
    pub fn with_output(mut self, output: JSONValue) -> Self {
        self.output = Some(output);
        self
    }

    /// Add an error result to the invocation.
    pub fn with_error(mut self, error: JSONValue) -> Self {
        self.output = Some(error);
        self.is_error = true;
        self
    }
}

#[cfg(test)]
#[path = "tool.test.rs"]
mod tests;
