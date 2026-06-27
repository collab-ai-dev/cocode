//! Extract reasoning content from assistant content parts.
//!
//! This module provides utilities for extracting reasoning/thinking content
//! from model responses.

use vercel_ai_provider::AssistantContentPart;

/// Extract all reasoning content from a slice of assistant content parts.
///
/// This function returns a vector of reasoning strings from `Reasoning` content parts.
/// Reasoning content typically represents the model's internal thinking process
/// (e.g., Claude's extended thinking, DeepSeek's reasoning tokens).
///
/// # Arguments
///
/// * `content` - A slice of assistant content parts.
///
/// # Returns
///
/// A `Vec<String>` containing all reasoning content.
///
/// # Example
///
/// ```ignore
/// use vercel_ai_provider::AssistantContentPart;
/// use vercel_ai::generate_text::extract_reasoning_content;
///
/// let content = vec![
///     AssistantContentPart::reasoning("Let me think about this..."),
///     AssistantContentPart::text("The answer is 42."),
/// ];
///
/// let reasoning = extract_reasoning_content(&content);
/// assert_eq!(reasoning, vec!["Let me think about this..."]);
/// ```
pub fn extract_reasoning_content(content: &[AssistantContentPart]) -> Vec<String> {
    content
        .iter()
        .filter_map(|part| match part {
            AssistantContentPart::Reasoning(r) => Some(r.text.clone()),
            _ => None,
        })
        .collect()
}

/// Extract concatenated reasoning content as a single string.
///
/// This function joins all reasoning content with newlines.
///
/// # Arguments
///
/// * `content` - A slice of assistant content parts.
///
/// # Returns
///
/// A `String` containing all reasoning content joined by newlines.
pub fn extract_reasoning_text(content: &[AssistantContentPart]) -> String {
    extract_reasoning_content(content).join("\n")
}

/// Check if any reasoning content is present.
///
/// # Arguments
///
/// * `content` - A slice of assistant content parts.
///
/// # Returns
///
/// `true` if any `Reasoning` content part is present, `false` otherwise.
pub fn has_reasoning_content(content: &[AssistantContentPart]) -> bool {
    content
        .iter()
        .any(|part| matches!(part, AssistantContentPart::Reasoning(_)))
}

/// Extract reasoning content with metadata.
///
/// This function returns both the reasoning strings and the total character count.
///
/// # Arguments
///
/// * `content` - A slice of assistant content parts.
///
/// # Returns
///
/// A tuple containing:
/// * A vector of reasoning strings
/// * The total character count of all reasoning content
pub fn extract_reasoning_with_stats(content: &[AssistantContentPart]) -> (Vec<String>, usize) {
    let reasoning = extract_reasoning_content(content);
    let char_count = reasoning.iter().map(std::string::String::len).sum();
    (reasoning, char_count)
}

#[cfg(test)]
#[path = "extract_reasoning_content.test.rs"]
mod tests;
