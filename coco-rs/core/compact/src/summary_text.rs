//! Compact-specific text extraction for summary prompts.
//!
//! All token *estimation* lives in [`coco_messages`] —
//! [`coco_messages::estimate_message_tokens`],
//! [`coco_messages::estimate_tokens_for_messages`],
//! [`coco_messages::estimate_tokens_for_messages_conservative`],
//! [`coco_messages::estimate_tool_result_message_tokens`],
//! [`coco_messages::estimate_text_tokens`], and the
//! [`coco_messages::MessageHistory::tokens_with_last_usage`] method.
//!
//! This module owns ONLY [`extract_message_text`] — converts a message
//! into the plain-text prose that the compact-summary LLM consumes.
//! The formatting decisions here (`[system]` placeholder, `[tool: name]`
//! placeholder for assistant tool calls) are compact-specific UX.

use coco_messages::AssistantContent;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::ToolContent;
use coco_messages::UserContent;

/// Extract all text content from a message (for building summary prompts).
pub fn extract_message_text(msg: &Message) -> Option<String> {
    match msg {
        Message::User(u) => extract_llm_message_text(&u.message),
        Message::Assistant(a) => extract_llm_message_text(&a.message),
        Message::ToolResult(tr) => extract_llm_message_text(&tr.message),
        Message::System(_) => Some("[system]".to_string()),
        Message::Attachment(a) => a
            .as_api_message()
            .and_then(extract_llm_message_text)
            .or_else(|| {
                let text = a.as_text_for_display();
                if text.is_empty() { None } else { Some(text) }
            }),
        _ => None,
    }
}

fn extract_llm_message_text(msg: &LlmMessage) -> Option<String> {
    match msg {
        LlmMessage::System { content, .. } | LlmMessage::Developer { content, .. } => {
            let texts: Vec<&str> = content
                .iter()
                .filter_map(|p| match p {
                    UserContent::Text(t) => Some(t.text.as_str()),
                    _ => None,
                })
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        }
        LlmMessage::User { content, .. } => {
            let texts: Vec<&str> = content
                .iter()
                .filter_map(|p| match p {
                    UserContent::Text(t) => Some(t.text.as_str()),
                    _ => None,
                })
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        }
        LlmMessage::Assistant { content, .. } => {
            // Walk content in emission order; emit text chunks and
            // `[tool: <name>]` placeholder lines for tool calls so the
            // compaction LLM sees where tools were invoked relative to
            // surrounding text. Otherwise interleaved
            // `[Text(A), ToolCall, Text(B)]` collapses to `"A\nB"` and
            // tool actions get silently misattributed in the summary.
            let mut chunks: Vec<String> = Vec::new();
            for p in content {
                match p {
                    AssistantContent::Text(t) if !t.text.is_empty() => {
                        chunks.push(t.text.clone());
                    }
                    AssistantContent::ToolCall(tc) => {
                        chunks.push(format!("[tool: {}]", tc.tool_name));
                    }
                    _ => {}
                }
            }
            if chunks.is_empty() {
                None
            } else {
                Some(chunks.join("\n"))
            }
        }
        LlmMessage::Tool { content, .. } => {
            let texts: Vec<String> = content
                .iter()
                .filter_map(|p| match p {
                    ToolContent::ToolResult(tr) => Some(format!(
                        "tool_use_id={}: {}",
                        tr.tool_call_id,
                        tool_result_text(&tr.output)
                    )),
                    _ => None,
                })
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        }
    }
}

fn tool_result_text(output: &coco_llm_types::ToolResultContent) -> String {
    match output {
        coco_llm_types::ToolResultContent::Text { value, .. } => value.clone(),
        coco_llm_types::ToolResultContent::Json { value, .. } => value.to_string(),
        coco_llm_types::ToolResultContent::Content { value, .. } => value
            .iter()
            .filter_map(|p| match p {
                coco_llm_types::ToolResultContentPart::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        coco_llm_types::ToolResultContent::ExecutionDenied { reason, .. } => {
            reason.clone().unwrap_or_default()
        }
        coco_llm_types::ToolResultContent::ErrorText { value, .. } => value.clone(),
        coco_llm_types::ToolResultContent::ErrorJson { value, .. } => value.to_string(),
    }
}

#[cfg(test)]
#[path = "summary_text.test.rs"]
mod tests;
