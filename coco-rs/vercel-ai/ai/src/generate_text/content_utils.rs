//! Shared content extraction utilities.
//!
//! This module consolidates the duplicated `extract_text`, `extract_reasoning`,
//! and `extract_tool_calls` functions that were scattered across
//! `generate_text_result.rs`, `generate_text.rs`, `stream_text.rs`, and `step_result.rs`.

use vercel_ai_provider::AssistantContentPart;

use super::generate_text_result::ToolCall;
use super::reasoning_output::ReasoningOutput;

/// Extract concatenated text from content parts.
pub fn extract_text(content: &[AssistantContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            AssistantContentPart::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Extract reasoning strings from content parts.
pub fn extract_reasoning(content: &[AssistantContentPart]) -> Vec<String> {
    content
        .iter()
        .filter_map(|part| match part {
            AssistantContentPart::Reasoning(r) => Some(r.text.clone()),
            _ => None,
        })
        .collect()
}

/// Extract structured reasoning outputs from content parts.
pub fn extract_reasoning_outputs(content: &[AssistantContentPart]) -> Vec<ReasoningOutput> {
    content
        .iter()
        .filter_map(|part| match part {
            AssistantContentPart::Reasoning(r) => {
                let mut output = ReasoningOutput::new(&r.text);
                if let Some(ref pm) = r.provider_metadata {
                    output.provider_metadata = Some(pm.clone());
                }
                Some(output)
            }
            _ => None,
        })
        .collect()
}

/// Extract tool calls from content parts.
pub fn extract_tool_calls(content: &[AssistantContentPart]) -> Vec<ToolCall> {
    content
        .iter()
        .filter_map(|part| match part {
            AssistantContentPart::ToolCall(tc) => Some(ToolCall::new(
                tc.tool_call_id.clone(),
                tc.tool_name.clone(),
                tc.input.clone(),
            )),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
#[path = "content_utils.test.rs"]
mod tests;
