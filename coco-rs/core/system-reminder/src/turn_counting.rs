//! Pure helpers for turning [`coco_messages::Message`] slices into the scalar
//! counters that generators read from [`GeneratorContext`].
//!
//! These functions are the bridge between raw message history and the
//! pre-computed fields a generator expects — the engine calls them once per
//! turn, stores the results on `GeneratorContext`, and generators read them
//! synchronously.
//!
//! Counting semantics:
//!
//! - Iterate messages backwards (newest first).
//! - "Assistant turns" exclude thinking-only messages (all-reasoning blocks).
//! - Stop counting once the matching tool_use is found; return the count.
//! - A tool never invoked returns the total number of assistant turns in
//!   history — i.e. "infinitely many turns ago" rounded to session length.
//!
//! [`GeneratorContext`]: crate::GeneratorContext

use std::borrow::Borrow;

use coco_messages::AssistantContent;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_types::AttachmentKind;
use coco_types::ToolName;

/// Task tools whose **invocation resets the task-reminder silence counter**.
///
/// Only `TaskCreate` / `TaskUpdate` are treated as "task management activity".
/// Read-only tools (`TaskGet`/`List`/`Stop`/`Output`) don't count — using
/// them does not acknowledge new or updated work.
///
/// Callers use this constant instead of a hand-rolled array so adding a new
/// mutation tool in [`ToolName`] flows through automatically.
pub const TASK_MANAGEMENT_TOOLS: &[ToolName] = &[ToolName::TaskCreate, ToolName::TaskUpdate];

/// Count assistant turns back from the end of history until we find an
/// assistant message that invoked *any* of `tools`.
///
/// Returns the count of assistant turns *before* the matching tool use
/// (i.e. if the very last assistant turn invoked the tool, returns 0).
///
/// Skips thinking-only messages (all-reasoning content blocks).
///
/// If the tool is never found, returns the total number of assistant turns
/// in `messages` (capped at `i32::MAX`). Callers treat this as "infinitely
/// many turns ago" since any threshold below session length will pass.
pub fn count_assistant_turns_since_any_tool<M: Borrow<Message>>(
    messages: &[M],
    tools: &[ToolName],
) -> i32 {
    let mut count: i32 = 0;
    for msg in messages.iter().rev() {
        let Message::Assistant(a) = msg.borrow() else {
            continue;
        };
        if is_thinking_only(&a.message) {
            continue;
        }
        if message_invokes_any_tool(&a.message, tools) {
            return count;
        }
        count = count.saturating_add(1);
    }
    count
}

/// Convenience wrapper: single typed tool.
pub fn count_assistant_turns_since_tool<M: Borrow<Message>>(messages: &[M], tool: ToolName) -> i32 {
    count_assistant_turns_since_any_tool(messages, &[tool])
}

/// Total number of assistant turns (non-thinking) in the history. Useful
/// upper bound for turn-gated logic and for tests.
pub fn total_assistant_turns<M: Borrow<Message>>(messages: &[M]) -> i32 {
    let mut count: i32 = 0;
    for msg in messages {
        if let Message::Assistant(a) = msg.borrow()
            && !is_thinking_only(&a.message)
        {
            count = count.saturating_add(1);
        }
    }
    count
}

/// Count **human turns**: non-meta `User` messages in history.
///
/// This is the canonical `turn_number` to feed
/// [`crate::SystemReminderOrchestrator::generate_all`] so plan-mode /
/// auto-mode throttle cadence is correct: human turns are counted, not LLM
/// iterations.
///
/// Tool-result rounds within one human turn do NOT advance the counter
/// because they aren't new `User` messages — each tool-call iteration
/// shares the originating human turn.
pub fn count_human_turns<M: Borrow<Message>>(messages: &[M]) -> i32 {
    let mut count: i32 = 0;
    for msg in messages {
        // Post-Phase-2: reminder-injected content is Message::Attachment,
        // so every `Message::User` is a genuine human turn.
        if matches!(msg.borrow(), Message::User(_)) {
            count = count.saturating_add(1);
        }
    }
    count
}

/// Count human turns since the most recent attachment of `kind`.
///
/// Scans backwards, counts human turns, stops at the marker attachment. If
/// the marker is absent, returns 0 so reminder logic stays disarmed.
///
/// Like [`count_human_turns`], this counts every `Message::User` and relies
/// on the invariant that reminder-injected content is `Message::Attachment`
/// and tool results are `Message::ToolResult` — so each `Message::User` is
/// a genuine human turn with no meta filtering needed.
pub fn count_human_turns_since_attachment<M: Borrow<Message>>(
    messages: &[M],
    kind: AttachmentKind,
) -> i32 {
    let mut count: i32 = 0;
    for msg in messages.iter().rev() {
        if matches!(msg.borrow(), Message::User(_)) {
            count = count.saturating_add(1);
        }
        if let Message::Attachment(attachment) = msg.borrow()
            && attachment.kind == kind
        {
            return count;
        }
    }
    0
}

/// Like [`count_human_turns_since_attachment`] but distinguishes "no prior
/// attachment" (`None`) from "0 human turns since the most recent one"
/// (`Some(0)`). Mirrors TS `getPlanModeAttachmentTurnCount`'s
/// `{ turnCount, foundPlanModeAttachment }` pair: the caller emits
/// unconditionally on the first plan/auto turn (`None`) and otherwise gates on
/// the turn count. This replaces the in-memory throttle's `last_generated_turn`
/// for the plan-mode / auto-mode cadence — history is the source of truth, so
/// cadence survives compaction (old markers vanish → re-emission is valid).
pub fn human_turns_since_attachment_opt<M: Borrow<Message>>(
    messages: &[M],
    kind: AttachmentKind,
) -> Option<i32> {
    let mut count: i32 = 0;
    for msg in messages.iter().rev() {
        if matches!(msg.borrow(), Message::User(_)) {
            count = count.saturating_add(1);
        }
        if let Message::Attachment(attachment) = msg.borrow()
            && attachment.kind == kind
        {
            return Some(count);
        }
    }
    None
}

/// Count attachments of `count_kind` scanning backwards, stopping at the most
/// recent `reset_kind` marker (exclusive). Used for the plan-mode / auto-mode
/// Full-vs-Sparse cycle: the Nth attachment since the last exit is Full.
/// Mirrors TS `countPlanModeAttachmentsSinceLastExit`. Re-entering plan/auto
/// mode (a `reset_kind` exit banner in history) restarts the Full/Sparse
/// cycle, and compaction naturally resets it too.
pub fn count_attachments_since_attachment<M: Borrow<Message>>(
    messages: &[M],
    count_kind: AttachmentKind,
    reset_kind: AttachmentKind,
) -> i32 {
    let mut count: i32 = 0;
    for msg in messages.iter().rev() {
        if let Message::Attachment(attachment) = msg.borrow() {
            if attachment.kind == reset_kind {
                break;
            }
            if attachment.kind == count_kind {
                count = count.saturating_add(1);
            }
        }
    }
    count
}

/// Count assistant turns since the most recent attachment of `kind`.
///
/// Scans backwards, skips thinking-only assistant messages, stops at the
/// matching reminder attachment. If the marker is absent, returns `i32::MAX`
/// so the absence of a prior reminder does not suppress the first reminder.
pub fn count_assistant_turns_since_attachment<M: Borrow<Message>>(
    messages: &[M],
    kind: AttachmentKind,
) -> i32 {
    let mut count: i32 = 0;
    for msg in messages.iter().rev() {
        match msg.borrow() {
            Message::Assistant(a) if !is_thinking_only(&a.message) => {
                count = count.saturating_add(1);
            }
            Message::Attachment(attachment) if attachment.kind == kind => {
                return count;
            }
            _ => {}
        }
    }
    i32::MAX
}

/// Returns true when this assistant message has content and every content
/// part is a reasoning block. An empty-content message is treated as
/// non-thinking (empty messages are not skipped).
fn is_thinking_only(msg: &LlmMessage) -> bool {
    let LlmMessage::Assistant { content, .. } = msg else {
        return false;
    };
    if content.is_empty() {
        return false;
    }
    content.iter().all(|c| {
        matches!(
            c,
            AssistantContent::Reasoning(_) | AssistantContent::ReasoningFile(_)
        )
    })
}

fn message_invokes_any_tool(msg: &LlmMessage, tools: &[ToolName]) -> bool {
    let LlmMessage::Assistant { content, .. } = msg else {
        return false;
    };
    content.iter().any(|c| match c {
        AssistantContent::ToolCall(tc) => tools.iter().any(|t| tc.tool_name == t.as_str()),
        _ => false,
    })
}

#[cfg(test)]
#[path = "turn_counting.test.rs"]
mod tests;
