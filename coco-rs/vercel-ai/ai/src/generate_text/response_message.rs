//! Response message building utilities.
//!
//! This module provides utilities for building response messages
//! from model outputs.

use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FinishReason;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::ToolContentPart;
use vercel_ai_provider::ToolResultContent;
use vercel_ai_provider::ToolResultPart;
use vercel_ai_provider::Usage;

use super::generate_text_result::ToolResult;

/// Build an assistant message from content parts.
///
/// # Arguments
///
/// * `content` - The assistant content parts.
///
/// # Returns
///
/// A `LanguageModelV4Message` representing the assistant's response.
pub fn build_assistant_message(content: Vec<AssistantContentPart>) -> LanguageModelV4Message {
    LanguageModelV4Message::assistant(content)
}

/// Build an assistant message from text.
///
/// # Arguments
///
/// * `text` - The text content.
///
/// # Returns
///
/// A `LanguageModelV4Message` representing the assistant's response.
pub fn build_assistant_message_from_text(text: impl Into<String>) -> LanguageModelV4Message {
    LanguageModelV4Message::assistant(vec![AssistantContentPart::text(text.into())])
}

/// Build a tool result message.
///
/// # Arguments
///
/// * `tool_results` - The tool results to include in the message.
///
/// # Returns
///
/// A `LanguageModelV4Message` containing the tool results.
pub fn build_tool_result_message(tool_results: &[ToolResult]) -> LanguageModelV4Message {
    let content: Vec<ToolContentPart> = tool_results
        .iter()
        .map(|tr| {
            ToolContentPart::ToolResult(ToolResultPart::new(
                &tr.tool_call_id,
                &tr.tool_name,
                ToolResultContent::text(
                    serde_json::to_string(&tr.result).unwrap_or_else(|_| tr.result.to_string()),
                ),
            ))
        })
        .collect();

    LanguageModelV4Message::tool(content)
}

/// Build a tool result message from a single result.
///
/// # Arguments
///
/// * `tool_call_id` - The tool call ID.
/// * `tool_name` - The tool name.
/// * `result` - The tool result as JSON.
/// * `is_error` - Whether the result is an error.
///
/// # Returns
///
/// A `LanguageModelV4Message` containing the tool result.
pub fn build_single_tool_result_message(
    tool_call_id: impl Into<String>,
    tool_name: impl Into<String>,
    result: serde_json::Value,
    is_error: bool,
) -> LanguageModelV4Message {
    let tool_call_id = tool_call_id.into();
    let tool_name = tool_name.into();

    let content = if is_error {
        ToolResultContent::text(format!("Error: {result}"))
    } else {
        ToolResultContent::text(serde_json::to_string(&result).unwrap_or_default())
    };

    let part = ToolContentPart::ToolResult(ToolResultPart::new(&tool_call_id, &tool_name, content));

    LanguageModelV4Message::tool(vec![part])
}

/// Response message data.
///
/// Contains all the data needed to construct response messages.
#[derive(Debug, Clone)]
pub struct ResponseMessageData {
    /// The content parts from the response.
    pub content: Vec<AssistantContentPart>,
    /// The finish reason.
    pub finish_reason: FinishReason,
    /// Token usage.
    pub usage: Usage,
    /// Tool calls made during generation.
    pub tool_calls: Vec<super::generate_text_result::ToolCall>,
    /// Tool results from executed tools.
    pub tool_results: Vec<ToolResult>,
}

impl ResponseMessageData {
    /// Create new response message data.
    pub fn new(
        content: Vec<AssistantContentPart>,
        finish_reason: FinishReason,
        usage: Usage,
    ) -> Self {
        Self {
            content,
            finish_reason,
            usage,
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
        }
    }

    /// Add tool calls.
    pub fn with_tool_calls(
        mut self,
        tool_calls: Vec<super::generate_text_result::ToolCall>,
    ) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    /// Add tool results.
    pub fn with_tool_results(mut self, tool_results: Vec<ToolResult>) -> Self {
        self.tool_results = tool_results;
        self
    }

    /// Check if there are tool calls.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Check if there are tool results.
    pub fn has_tool_results(&self) -> bool {
        !self.tool_results.is_empty()
    }

    /// Build the assistant message.
    pub fn to_assistant_message(&self) -> LanguageModelV4Message {
        build_assistant_message(self.content.clone())
    }

    /// Build the tool result message if there are tool results.
    pub fn to_tool_result_message(&self) -> Option<LanguageModelV4Message> {
        if self.tool_results.is_empty() {
            None
        } else {
            Some(build_tool_result_message(&self.tool_results))
        }
    }

    /// Build all messages (assistant + tool results if any).
    pub fn to_messages(&self) -> Vec<LanguageModelV4Message> {
        let mut messages = vec![self.to_assistant_message()];
        if let Some(tool_msg) = self.to_tool_result_message() {
            messages.push(tool_msg);
        }
        messages
    }
}

#[cfg(test)]
#[path = "response_message.test.rs"]
mod tests;
