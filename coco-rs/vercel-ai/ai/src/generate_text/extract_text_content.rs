//! Extract text content from assistant content parts.
//!
//! This module provides utilities for extracting text from model responses.

use vercel_ai_provider::AssistantContentPart;

/// Extract all text content from a slice of assistant content parts.
///
/// This function concatenates all text from `Text` content parts,
/// ignoring other content types like reasoning, tool calls, etc.
///
/// # Arguments
///
/// * `content` - A slice of assistant content parts.
///
/// # Returns
///
/// A `String` containing all concatenated text content.
///
/// # Example
///
/// ```ignore
/// use vercel_ai_provider::AssistantContentPart;
/// use vercel_ai::generate_text::extract_text_content;
///
/// let content = vec![
///     AssistantContentPart::text("Hello, "),
///     AssistantContentPart::text("world!"),
/// ];
///
/// let text = extract_text_content(&content);
/// assert_eq!(text, "Hello, world!");
/// ```
pub fn extract_text_content(content: &[AssistantContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            AssistantContentPart::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Extract text content with metadata from assistant content parts.
///
/// This function returns both the concatenated text and information about
/// other content types present.
///
/// # Arguments
///
/// * `content` - A slice of assistant content parts.
///
/// # Returns
///
/// A tuple containing:
/// * The concatenated text content
/// * Whether any reasoning was present
/// * Whether any tool calls were present
pub fn extract_text_content_with_metadata(
    content: &[AssistantContentPart],
) -> (String, bool, bool) {
    let mut text = String::new();
    let mut has_reasoning = false;
    let mut has_tool_calls = false;

    for part in content {
        match part {
            AssistantContentPart::Text(t) => {
                text.push_str(&t.text);
            }
            AssistantContentPart::Reasoning(_) => {
                has_reasoning = true;
            }
            AssistantContentPart::ToolCall(_) => {
                has_tool_calls = true;
            }
            _ => {}
        }
    }

    (text, has_reasoning, has_tool_calls)
}

#[cfg(test)]
#[path = "extract_text_content.test.rs"]
mod tests;
