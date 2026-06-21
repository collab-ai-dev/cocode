//! Message predicates — 19 boolean checks used throughout the pipeline.

use crate::AssistantContent;
use crate::LlmMessage;
use crate::Message;
use crate::MessageKind;
use crate::StopReason;
use coco_types::ToolId;

pub fn is_user_message(msg: &Message) -> bool {
    msg.kind() == MessageKind::User
}

pub fn is_assistant_message(msg: &Message) -> bool {
    msg.kind() == MessageKind::Assistant
}

pub fn is_system_message(msg: &Message) -> bool {
    msg.kind() == MessageKind::System
}

pub fn is_tool_result(msg: &Message) -> bool {
    msg.kind() == MessageKind::ToolResult
}

pub fn is_progress_message(msg: &Message) -> bool {
    msg.kind() == MessageKind::Progress
}

pub fn is_tombstone(msg: &Message) -> bool {
    msg.kind() == MessageKind::Tombstone
}

/// Whether the message is hidden from UI but visible to model.
///
/// Post-Phase-2: `UserMessage` no longer carries `is_meta` — all
/// reminder-injected user content lives in `Message::Attachment`
/// with kind = `CriticalSystemReminder` (or a more specific reminder
/// kind). "Meta" is now purely an `Attachment`-layer concept.
pub fn is_meta_message(msg: &Message) -> bool {
    match msg {
        Message::Attachment(m) => m.kind.is_api_visible() && !m.kind.renders_in_transcript(),
        _ => false,
    }
}

/// Whether the message is not sent to API (client-side only).
pub fn is_virtual_message(msg: &Message) -> bool {
    match msg {
        Message::User(m) => m.is_virtual,
        _ => false,
    }
}

/// Whether the message is a compact summary.
pub fn is_compact_summary(msg: &Message) -> bool {
    match msg {
        Message::User(m) => m.is_compact_summary,
        _ => false,
    }
}

/// Whether an assistant message has tool calls in its content.
pub fn has_tool_calls(msg: &Message) -> bool {
    match msg {
        Message::Assistant(m) => match &m.message {
            LlmMessage::Assistant { content, .. } => content
                .iter()
                .any(|c| matches!(c, AssistantContent::ToolCall(_))),
            _ => false,
        },
        _ => false,
    }
}

/// Whether an assistant message stopped due to tool use.
pub fn stopped_for_tool_use(msg: &Message) -> bool {
    match msg {
        Message::Assistant(m) => m.stop_reason == Some(StopReason::ToolUse),
        _ => false,
    }
}

/// Whether an assistant message stopped due to max tokens.
pub fn stopped_for_max_tokens(msg: &Message) -> bool {
    match msg {
        Message::Assistant(m) => m.stop_reason == Some(StopReason::MaxTokens),
        _ => false,
    }
}

/// Whether a tool result is an error.
pub fn is_tool_error(msg: &Message) -> bool {
    match msg {
        Message::ToolResult(m) => m.is_error,
        _ => false,
    }
}

/// Whether a tool result is for a specific tool.
pub fn is_tool_result_for(msg: &Message, tool_id: &ToolId) -> bool {
    match msg {
        Message::ToolResult(m) => &m.tool_id == tool_id,
        _ => false,
    }
}

/// Whether a message has any text content.
pub fn has_text_content(msg: &Message) -> bool {
    match msg {
        Message::User(m) => match &m.message {
            LlmMessage::User { content, .. } => content
                .iter()
                .any(|c| matches!(c, crate::UserContent::Text(_))),
            _ => false,
        },
        Message::Assistant(m) => match &m.message {
            LlmMessage::Assistant { content, .. } => content
                .iter()
                .any(|c| matches!(c, AssistantContent::Text(_))),
            _ => false,
        },
        _ => false,
    }
}

/// Whether a message is an API error system message.
pub fn is_api_error_message(msg: &Message) -> bool {
    matches!(msg, Message::System(crate::SystemMessage::ApiError(_)))
}

/// Whether a message is an attachment.
pub fn is_attachment(msg: &Message) -> bool {
    msg.kind() == MessageKind::Attachment
}

/// Count tool calls in an assistant message.
pub fn tool_call_count(msg: &Message) -> usize {
    match msg {
        Message::Assistant(m) => match &m.message {
            LlmMessage::Assistant { content, .. } => content
                .iter()
                .filter(|c| matches!(c, AssistantContent::ToolCall(_)))
                .count(),
            _ => 0,
        },
        _ => 0,
    }
}

/// Count tool calls in the **last** assistant message of `messages`,
/// walking from the tail. Returns 0 when no assistant message exists.
///
/// Used by the session-memory natural-break gate and by compact's SM-first
/// short-circuit to decide whether the last turn ended on a tool
/// call (orphan-risk for downstream summaries).
pub fn count_tool_calls_in_last_assistant_turn<M: std::borrow::Borrow<Message>>(
    messages: &[M],
) -> i32 {
    for m in messages.iter().rev() {
        let msg = m.borrow();
        if matches!(msg, Message::Assistant(_)) {
            return tool_call_count(msg) as i32;
        }
    }
    0
}

/// Return the slice of `messages` that follows the most recent compact
/// boundary (either a `SystemMessage::CompactBoundary` or a synthesized
/// `is_compact_summary` user message). When no boundary exists, the
/// whole slice is returned.
///
/// Feeds the rename auto-name path and any other consumer that needs
/// "messages since the last compaction" without re-walking compact
/// state machinery.
pub fn messages_after_compact_boundary<M: std::borrow::Borrow<Message>>(messages: &[M]) -> &[M] {
    for (idx, m) in messages.iter().enumerate().rev() {
        let msg = m.borrow();
        let is_boundary = matches!(
            msg,
            Message::System(crate::SystemMessage::CompactBoundary(_))
        ) || is_compact_summary(msg);
        if is_boundary {
            return &messages[idx + 1..];
        }
    }
    messages
}

#[cfg(test)]
#[path = "predicates.test.rs"]
mod tests;
