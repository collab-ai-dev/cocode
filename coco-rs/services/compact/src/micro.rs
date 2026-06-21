//! Basic micro-compaction: clear old tool results to free context.
//!
//! Replaces old tool result content with a placeholder. Preserves message
//! structure (tool_use/tool_result pairing) — does NOT tombstone messages.

use coco_messages::AssistantContent;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_types::ToolName;

use crate::types::CLEARED_TOOL_RESULT_MESSAGE;
use crate::types::MicrocompactResult;

/// Tools whose results can be cleared during micro-compaction.
/// Exactly: FileRead, ...SHELL_TOOL_NAMES, Grep, Glob, WebSearch, WebFetch,
/// FileEdit, FileWrite.
pub const COMPACTABLE_TOOLS: &[ToolName] = &[
    ToolName::Read,
    ToolName::Bash,
    ToolName::PowerShell,
    ToolName::Grep,
    ToolName::Glob,
    ToolName::WebSearch,
    ToolName::WebFetch,
    ToolName::Edit,
    ToolName::Write,
];

/// Walk messages in encounter order and collect tool_use IDs whose tool name
/// is compactable.
pub fn collect_compactable_tool_ids(messages: &[Message]) -> Vec<String> {
    let mut ids = Vec::new();
    for msg in messages {
        let Message::Assistant(asst) = msg else {
            continue;
        };
        let LlmMessage::Assistant { content, .. } = &asst.message else {
            continue;
        };
        for part in content {
            let AssistantContent::ToolCall(tc) = part else {
                continue;
            };
            if tool_name_is_compactable(&tc.tool_name) {
                ids.push(tc.tool_call_id.clone());
            }
        }
    }
    ids
}

/// Perform micro-compaction: clear old tool results to free context.
///
/// Replaces tool result content with `[Old tool result content cleared]` while
/// preserving the message structure required by the API. Only clears results
/// from `COMPACTABLE_TOOLS`. Keeps the **last `keep_recent` compactable
/// tool_use_ids** intact (counts tool calls, not messages).
pub fn micro_compact(messages: &mut [Message], keep_recent: usize) -> MicrocompactResult {
    // Floor at 1: slice(-0) would return all, leaving the model with zero
    // working context.
    let keep_recent = keep_recent.max(1);
    let compactable_ids = collect_compactable_tool_ids(messages);
    let total = compactable_ids.len();
    let keep_set: std::collections::HashSet<&str> = compactable_ids
        .iter()
        .skip(total.saturating_sub(keep_recent))
        .map(String::as_str)
        .collect();
    let clear_set: std::collections::HashSet<&str> = compactable_ids
        .iter()
        .map(String::as_str)
        .filter(|id| !keep_set.contains(id))
        .collect();

    let mut cleared: i32 = 0;
    let mut tokens_freed: i64 = 0;

    for msg in messages.iter_mut() {
        let Message::ToolResult(tr) = msg else {
            continue;
        };

        if !clear_set.contains(tr.tool_use_id.as_str()) {
            continue;
        }

        if is_already_cleared(tr) {
            continue;
        }

        let est_tokens = coco_messages::estimate_tool_result_message_tokens(tr);
        tr.message = LlmMessage::Tool {
            content: vec![coco_messages::ToolContent::ToolResult(
                coco_messages::ToolResultContent {
                    tool_call_id: tr.tool_use_id.clone(),
                    tool_name: String::new(),
                    output: coco_llm_types::ToolResultContent::text(CLEARED_TOOL_RESULT_MESSAGE),
                    is_error: false,
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        };

        tokens_freed += est_tokens;
        cleared += 1;
    }

    if cleared > 0 {
        tracing::debug!(
            cleared,
            tokens_freed,
            keep_recent,
            "micro-compaction cleared old tool results"
        );
    }

    MicrocompactResult {
        messages_cleared: cleared,
        tokens_saved_estimate: tokens_freed,
        was_time_triggered: false,
    }
}

/// Check whether a tool name string is in the compactable set.
fn tool_name_is_compactable(name: &str) -> bool {
    COMPACTABLE_TOOLS.iter().any(|t| t.as_str() == name)
}

/// Time-based micro-compaction: when the gap since the last assistant
/// message exceeds the configured threshold, content-clear all but the most
/// recent N compactable tool results.
///
/// Caller is responsible for evaluating
/// [`crate::auto_trigger::evaluate_time_based_trigger`] first; this function
/// performs the action when the trigger fires.
///
/// Returns the [`MicrocompactResult`] with `was_time_triggered = true` when
/// at least one tool result was cleared; otherwise `None` (no work to do).
pub fn time_based_microcompact(
    messages: &mut [Message],
    trigger: &crate::auto_trigger::TimeBasedTrigger,
) -> Option<MicrocompactResult> {
    let result = micro_compact(messages, trigger.config.keep_recent.max(1) as usize);
    if result.messages_cleared == 0 {
        return None;
    }
    Some(MicrocompactResult {
        messages_cleared: result.messages_cleared,
        tokens_saved_estimate: result.tokens_saved_estimate,
        was_time_triggered: true,
    })
}

/// Check if a tool result has already been cleared.
fn is_already_cleared(tr: &coco_messages::ToolResultMessage) -> bool {
    if let LlmMessage::Tool { content, .. } = &tr.message
        && content.len() == 1
        && let coco_messages::ToolContent::ToolResult(part) = &content[0]
        && let coco_llm_types::ToolResultContent::Text { value, .. } = &part.output
    {
        return value == CLEARED_TOOL_RESULT_MESSAGE;
    }
    false
}

#[cfg(test)]
#[path = "micro.test.rs"]
mod tests;
