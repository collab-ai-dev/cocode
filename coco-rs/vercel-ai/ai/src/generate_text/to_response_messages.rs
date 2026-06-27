//! Build response messages from tool results.
//!
//! This module provides functionality for converting tool execution results
//! into response messages that can be added to the conversation history.

use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::ToolCallPart;
use vercel_ai_provider::ToolContentPart;
use vercel_ai_provider::ToolResultContent;
use vercel_ai_provider::ToolResultPart;

use super::generate_text_result::ToolCall;
use super::generate_text_result::ToolResult;

/// Response messages built from tool results.
#[derive(Debug, Clone)]
pub struct ResponseMessages {
    /// The assistant message with tool calls.
    pub assistant_message: LanguageModelV4Message,
    /// The tool result message (if any tool results).
    pub tool_message: Option<LanguageModelV4Message>,
}

impl ResponseMessages {
    /// Create new response messages.
    pub fn new(
        assistant_message: LanguageModelV4Message,
        tool_message: Option<LanguageModelV4Message>,
    ) -> Self {
        Self {
            assistant_message,
            tool_message,
        }
    }

    /// Convert to a vector of messages.
    pub fn to_vec(&self) -> Vec<LanguageModelV4Message> {
        let mut messages = vec![self.assistant_message.clone()];
        if let Some(tool_msg) = &self.tool_message {
            messages.push(tool_msg.clone());
        }
        messages
    }
}

/// Build response messages from assistant content and tool results.
///
/// # Arguments
///
/// * `content` - The assistant content parts.
/// * `tool_results` - The tool results.
///
/// # Returns
///
/// Response messages containing the assistant message and tool message.
pub fn to_response_messages(
    content: Vec<AssistantContentPart>,
    tool_results: &[ToolResult],
) -> ResponseMessages {
    let assistant_message = LanguageModelV4Message::assistant(content);

    let tool_message = if tool_results.is_empty() {
        None
    } else {
        Some(build_tool_result_message(tool_results))
    };

    ResponseMessages::new(assistant_message, tool_message)
}

/// Build response messages from tool calls and tool results.
///
/// This creates the assistant message with tool calls and the tool result message.
///
/// # Arguments
///
/// * `tool_calls` - The tool calls made by the assistant.
/// * `tool_results` - The results from executing those tools.
///
/// # Returns
///
/// Response messages containing the assistant and tool messages.
pub fn to_response_messages_from_tool_calls(
    tool_calls: &[ToolCall],
    tool_results: &[ToolResult],
) -> ResponseMessages {
    // Build assistant content with tool calls
    let content: Vec<AssistantContentPart> = tool_calls
        .iter()
        .map(|tc| {
            AssistantContentPart::ToolCall(ToolCallPart::new(
                &tc.tool_call_id,
                &tc.tool_name,
                tc.args.clone(),
            ))
        })
        .collect();

    to_response_messages(content, tool_results)
}

/// Build a tool result message from tool results.
///
/// # Arguments
///
/// * `tool_results` - The tool results to include.
///
/// # Returns
///
/// A tool message containing the results.
pub fn build_tool_result_message(tool_results: &[ToolResult]) -> LanguageModelV4Message {
    let content: Vec<ToolContentPart> = tool_results
        .iter()
        .map(|tr| {
            let result_content = if tr.is_error {
                ToolResultContent::text(format!(
                    "Error: {}",
                    serde_json::to_string(&tr.result).unwrap_or_default()
                ))
            } else {
                ToolResultContent::text(
                    serde_json::to_string(&tr.result).unwrap_or_else(|_| tr.result.to_string()),
                )
            };

            ToolContentPart::ToolResult(ToolResultPart::new(
                &tr.tool_call_id,
                &tr.tool_name,
                result_content,
            ))
        })
        .collect();

    LanguageModelV4Message::tool(content)
}

/// Build response messages from text and tool results.
///
/// This is used when the assistant response contains both text and tool calls.
///
/// # Arguments
///
/// * `text` - The text content.
/// * `tool_calls` - The tool calls.
/// * `tool_results` - The tool results.
///
/// # Returns
///
/// Response messages with combined content.
pub fn to_response_messages_with_text(
    text: &str,
    tool_calls: &[ToolCall],
    tool_results: &[ToolResult],
) -> ResponseMessages {
    // Build assistant content with text and tool calls
    let mut content = Vec::new();

    // Add text part
    if !text.is_empty() {
        content.push(AssistantContentPart::text(text));
    }

    // Add tool call parts
    for tc in tool_calls {
        content.push(AssistantContentPart::ToolCall(ToolCallPart::new(
            &tc.tool_call_id,
            &tc.tool_name,
            tc.args.clone(),
        )));
    }

    to_response_messages(content, tool_results)
}

/// Build a single assistant message from content.
///
/// # Arguments
///
/// * `content` - The assistant content.
///
/// # Returns
///
/// An assistant message.
pub fn build_assistant_response(content: Vec<AssistantContentPart>) -> LanguageModelV4Message {
    LanguageModelV4Message::assistant(content)
}

/// Build a text-only assistant message.
///
/// # Arguments
///
/// * `text` - The text content.
///
/// # Returns
///
/// An assistant message with text only.
pub fn build_text_response(text: impl Into<String>) -> LanguageModelV4Message {
    LanguageModelV4Message::assistant(vec![AssistantContentPart::text(text.into())])
}
