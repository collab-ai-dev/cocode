//! Enhanced micro-compaction: budget-aware tool result clearing, file-unchanged
//! stub removal, and thinking block compaction.
//!
//! This module extends the basic micro-compaction (`micro.rs`) with:
//! - Token-budget-aware clearing (stop once enough tokens are freed)
//! - Removal of "[file unchanged]" placeholder stubs
//! - Compaction of thinking/reasoning blocks from old turns
//! - **In-place** versions of the API-native clear strategies — these
//!   mutate messages directly (and break prompt cache as a result). For
//!   the cache-preserving server-side variant, build a config via
//!   [`crate::api_compact::get_api_context_management`] and pass it to the
//!   provider in `ProviderOptions`.

use coco_messages::AssistantContent;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_types::ToolName;

use crate::types::CLEARED_TOOL_RESULT_MESSAGE;
use crate::types::MicrocompactResult;

/// Configuration for budget-aware micro-compaction.
#[derive(Debug, Clone, Default)]
pub struct MicroCompactBudgetConfig {
    /// Maximum tokens to free. Compaction stops once this is met.
    pub tokens_to_free: i64,
    /// Number of recent messages to always keep intact.
    pub keep_recent: usize,
    /// Tools whose results should never be cleared.
    ///
    /// Builtin tools should be supplied via [`ToolName`] for compile-time
    /// verification (CLAUDE.md "no hardcoded strings for closed sets").
    /// MCP / custom tool identifiers can be supplied via `exclude_tool_strs`.
    pub exclude_tools: Vec<ToolName>,
    /// Additional non-builtin tool identifiers to exclude (matched by
    /// substring against `ToolId::to_string()` to keep parity with the
    /// previous string-based API).
    pub exclude_tool_strs: Vec<String>,
}

/// Clear old tool results within a token budget.
///
/// Walks messages oldest-first, replacing tool result content with a
/// placeholder until `tokens_to_free` tokens have been reclaimed. Skips
/// the most recent `keep_recent` messages and any tools in the exclude list.
pub fn micro_compact_with_budget(
    messages: &mut [Message],
    config: &MicroCompactBudgetConfig,
) -> MicrocompactResult {
    let total = messages.len();
    let cutoff = total.saturating_sub(config.keep_recent);

    let mut tokens_freed: i64 = 0;
    let mut cleared: i32 = 0;

    for msg in messages.iter_mut().take(cutoff) {
        if tokens_freed >= config.tokens_to_free {
            break;
        }
        let Message::ToolResult(tr) = msg else {
            continue;
        };

        // Check if this tool is excluded — builtin (typed) and string lists.
        let id_str = tr.tool_id.to_string();
        let excluded = config.exclude_tools.iter().any(|t| t.as_str() == id_str)
            || config
                .exclude_tool_strs
                .iter()
                .any(|ex| id_str.contains(ex));
        if excluded {
            continue;
        }

        let est_tokens = coco_messages::estimate_tool_result_message_tokens(tr);
        if est_tokens <= 10 {
            // Already tiny, not worth clearing
            continue;
        }

        // Replace with cleared placeholder
        tr.message = coco_messages::LlmMessage::Tool {
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

    MicrocompactResult {
        messages_cleared: cleared,
        tokens_saved_estimate: tokens_freed,
        was_time_triggered: false,
    }
}

/// Sentinel text inserted by file-edit tools when no changes were made.
const FILE_UNCHANGED_STUB: &str = "[file unchanged]";

/// Remove "[file unchanged]" placeholder stubs from tool results.
///
/// When file-edit tools detect no changes, they leave a stub. Over many turns
/// these accumulate without value. This function replaces matching tool results
/// with a minimal placeholder, freeing tokens.
pub fn clear_file_unchanged_stubs(messages: &mut [Message]) -> MicrocompactResult {
    let mut cleared: i32 = 0;
    let mut tokens_freed: i64 = 0;

    for msg in messages.iter_mut() {
        let Message::ToolResult(tr) = msg else {
            continue;
        };

        if !tool_result_contains_text(tr, FILE_UNCHANGED_STUB) {
            continue;
        }

        let est_tokens = coco_messages::estimate_tool_result_message_tokens(tr);
        tr.message = coco_messages::LlmMessage::Tool {
            content: vec![coco_messages::ToolContent::ToolResult(
                coco_messages::ToolResultContent {
                    tool_call_id: tr.tool_use_id.clone(),
                    tool_name: String::new(),
                    output: coco_llm_types::ToolResultContent::text(
                        "[file unchanged - stub cleared]",
                    ),
                    is_error: false,
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        };

        tokens_freed += est_tokens.saturating_sub(8); // placeholder is ~8 tokens
        cleared += 1;
    }

    MicrocompactResult {
        messages_cleared: cleared,
        tokens_saved_estimate: tokens_freed,
        was_time_triggered: false,
    }
}

/// Remove thinking/reasoning blocks from old assistant turns.
///
/// Thinking blocks are valuable during the current reasoning chain but become
/// dead weight in older turns. This function removes them from all assistant
/// messages except the most recent `keep_recent_turns`.
pub fn compact_thinking_blocks(
    messages: &mut [Message],
    keep_recent_turns: usize,
) -> MicrocompactResult {
    // Find assistant message indices
    let assistant_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter_map(|(i, m)| matches!(m, Message::Assistant(_)).then_some(i))
        .collect();

    let cutoff_count = assistant_indices.len().saturating_sub(keep_recent_turns);
    let indices_to_compact = &assistant_indices[..cutoff_count];

    let mut cleared: i32 = 0;
    let mut tokens_freed: i64 = 0;

    for &idx in indices_to_compact {
        let Message::Assistant(ref mut asst) = messages[idx] else {
            continue;
        };

        let coco_messages::LlmMessage::Assistant {
            ref mut content, ..
        } = asst.message
        else {
            continue;
        };

        let original_len = content.len();
        let mut removed_tokens: i64 = 0;

        content.retain(|part| {
            match part {
                coco_messages::AssistantContent::Reasoning(r) => {
                    removed_tokens += (r.text.len() as i64) / 4;
                    false // Remove thinking blocks
                }
                _ => true, // Keep everything else
            }
        });

        let removed = (original_len - content.len()) as i32;
        if removed > 0 {
            cleared += removed;
            tokens_freed += removed_tokens;
        }
    }

    MicrocompactResult {
        messages_cleared: cleared,
        tokens_saved_estimate: tokens_freed,
        was_time_triggered: false,
    }
}

/// In-place client-side analog of the API-native `clear_tool_uses_20250919`
/// strategy.
///
/// Walks assistant messages oldest-first and replaces tool-call `input`
/// fields with an empty JSON object, preserving the tool-call structure
/// so the API still sees matching call/result pairs. Skips the most
/// recent `keep_recent_count` assistant messages and any tools named in
/// `exclude_tools`.
///
/// **This breaks the prompt cache** because it modifies what was previously
/// sent. Prefer [`crate::api_compact::get_api_context_management`] when
/// the provider supports server-side context editing.
pub fn clear_tool_uses_inplace(
    messages: &mut [Message],
    keep_recent_count: usize,
    exclude_tools: &[ToolName],
) -> MicrocompactResult {
    let assistant_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter_map(|(i, m)| matches!(m, Message::Assistant(_)).then_some(i))
        .collect();

    let cutoff_count = assistant_indices.len().saturating_sub(keep_recent_count);
    let indices_to_clear = &assistant_indices[..cutoff_count];

    let mut cleared: i32 = 0;
    let mut tokens_freed: i64 = 0;

    for &idx in indices_to_clear {
        let Message::Assistant(ref mut asst) = messages[idx] else {
            continue;
        };

        let LlmMessage::Assistant {
            ref mut content, ..
        } = asst.message
        else {
            continue;
        };

        for part in content.iter_mut() {
            let AssistantContent::ToolCall(ref mut tc) = *part else {
                continue;
            };

            if exclude_tools.iter().any(|t| t.as_str() == tc.tool_name) {
                continue;
            }

            let input_str = tc.input.to_string();
            let est_tokens = (input_str.len() as i64) / 4;
            if est_tokens <= 5 {
                continue;
            }

            tc.input = serde_json::Value::Object(serde_json::Map::new());
            tokens_freed += est_tokens;
            cleared += 1;
        }
    }

    MicrocompactResult {
        messages_cleared: cleared,
        tokens_saved_estimate: tokens_freed,
        was_time_triggered: false,
    }
}

/// In-place client-side analog of the API-native `clear_thinking_20251015`
/// strategy.
///
/// Removes `Reasoning` content parts entirely, freeing tokens used by
/// chain-of-thought no longer needed. Preserves text and tool-call parts.
///
/// **This breaks the prompt cache.** Prefer the API-native config builder
/// when the provider supports it.
pub fn clear_thinking_inplace(messages: &mut [Message]) -> MicrocompactResult {
    let mut cleared: i32 = 0;
    let mut tokens_freed: i64 = 0;

    for msg in messages.iter_mut() {
        let Message::Assistant(ref mut asst) = *msg else {
            continue;
        };

        let LlmMessage::Assistant {
            ref mut content, ..
        } = asst.message
        else {
            continue;
        };

        let original_len = content.len();
        let mut removed_tokens: i64 = 0;

        content.retain(|part| match part {
            AssistantContent::Reasoning(r) => {
                removed_tokens += (r.text.len() as i64) / 4;
                false
            }
            _ => true,
        });

        let removed = (original_len - content.len()) as i32;
        if removed > 0 {
            cleared += removed;
            tokens_freed += removed_tokens;
        }
    }

    MicrocompactResult {
        messages_cleared: cleared,
        tokens_saved_estimate: tokens_freed,
        was_time_triggered: false,
    }
}

/// Check if a tool result message contains specific text (walks content parts).
fn tool_result_contains_text(tr: &coco_messages::ToolResultMessage, needle: &str) -> bool {
    let coco_messages::LlmMessage::Tool { content, .. } = &tr.message else {
        return false;
    };
    for part in content {
        let coco_messages::ToolContent::ToolResult(result) = part else {
            continue;
        };
        match &result.output {
            coco_llm_types::ToolResultContent::Text { value, .. } => {
                if value.contains(needle) {
                    return true;
                }
            }
            coco_llm_types::ToolResultContent::Content { value, .. } => {
                for sub in value {
                    if let coco_llm_types::ToolResultContentPart::Text { text, .. } = sub
                        && text.contains(needle)
                    {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
#[path = "micro_advanced.test.rs"]
mod tests;
