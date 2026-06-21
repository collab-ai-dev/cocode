//! Convert prompts to language model format.
//!
//! This module provides functions for converting `Prompt` and `ModelMessage`
//! types to the `LanguageModelV4Message` format used by providers.

use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4Prompt;
use vercel_ai_provider::ToolContentPart;

use crate::error::MissingToolResultsError;

/// Convert a standardized prompt to a language model prompt.
///
/// This function handles:
/// - System messages
/// - User messages
/// - Assistant messages
/// - Tool messages
/// - Tool call/result pairing validation
pub fn convert_to_language_model_prompt(
    system: Option<Vec<LanguageModelV4Message>>,
    messages: Vec<LanguageModelV4Message>,
) -> Result<LanguageModelV4Prompt, MissingToolResultsError> {
    let mut result = Vec::new();

    // Add system messages
    if let Some(system_messages) = system {
        result.extend(system_messages);
    }

    // Track tool calls that need results
    let mut pending_tool_calls: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    // Process messages
    for message in messages {
        match &message {
            LanguageModelV4Message::Assistant { content, .. } => {
                // Add tool call IDs to pending set
                for part in content {
                    if let AssistantContentPart::ToolCall(tool_call) = part {
                        pending_tool_calls.insert(tool_call.tool_call_id.clone());
                    }
                }
                result.push(message);
            }
            LanguageModelV4Message::Tool { content, .. } => {
                // Remove tool call IDs from pending set
                for part in content {
                    if let ToolContentPart::ToolResult(tool_result) = part {
                        pending_tool_calls.remove(&tool_result.tool_call_id);
                    }
                }
                result.push(message);
            }
            LanguageModelV4Message::User { .. }
            | LanguageModelV4Message::System { .. }
            | LanguageModelV4Message::Developer { .. } => {
                // Check for missing tool results before user/system messages
                if !pending_tool_calls.is_empty() {
                    return Err(MissingToolResultsError::new(
                        pending_tool_calls.into_iter().collect(),
                    ));
                }
                result.push(message);
            }
        }
    }

    // Check for any remaining missing tool results
    if !pending_tool_calls.is_empty() {
        return Err(MissingToolResultsError::new(
            pending_tool_calls.into_iter().collect(),
        ));
    }

    Ok(result)
}

/// Convert a single message to language model format.
pub fn convert_to_language_model_message(
    message: LanguageModelV4Message,
) -> LanguageModelV4Message {
    message
}

/// Combine consecutive tool messages into a single tool message.
///
/// This is useful for providers that prefer combined tool messages.
pub fn combine_tool_messages(messages: LanguageModelV4Prompt) -> LanguageModelV4Prompt {
    let mut combined: Vec<LanguageModelV4Message> = Vec::new();

    for message in messages {
        match (&message, combined.last_mut()) {
            (
                LanguageModelV4Message::Tool { content, .. },
                Some(LanguageModelV4Message::Tool {
                    content: last_content,
                    ..
                }),
            ) => {
                // Combine tool messages
                last_content.extend(content.clone());
            }
            _ => {
                combined.push(message);
            }
        }
    }

    combined
}

#[cfg(test)]
#[path = "convert.test.rs"]
mod tests;
