//! Transcript filtering for agent resume — strip incomplete or corrupted
//! messages so the model doesn't see orphaned `tool_use` blocks or empty
//! assistant turns when picking up an interrupted session.
//!
//! `tools/AgentTool/resumeAgent.ts` — `filterOrphanedThinkingOnlyMessages`
//! + `filterUnresolvedToolUses` + `filterWhitespaceOnlyAssistantMessages`.
//!
//! Pure logic over the typed message family — no tokio, no HTTP, no JSON
//! shape inspection. The runner (`root/coordinator`) calls
//! `filter_transcript` after deserializing JSONL transcript lines into
//! typed `Arc<Message>`.

use std::sync::Arc;

use coco_llm_types::AssistantContentPart;
use coco_llm_types::LlmMessage;
use coco_types::messages::Message;

/// Filter out incomplete or corrupted messages from a transcript.
///
/// Three filter passes (`filterOrphanedThinkingOnlyMessages` +
/// `filterUnresolvedToolUses` +
/// `filterWhitespaceOnlyAssistantMessages`):
///
/// 1. Drop assistant turns whose content is all whitespace text.
/// 2. Drop assistant turns whose content is reasoning-only (no text,
///    no tool call) — the resumed model has no use for the parent's
///    private thinking with no follow-through.
/// 3. Strip trailing assistant turns whose `ToolCall`s have no matching
///    `Message::ToolResult` further on — the resumed conversation
///    can't honour them so they'd just confuse the model.
///
/// Non-assistant entries (User, ToolResult, System, Attachment,
/// Progress, Tombstone) pass through unchanged. `Arc` clones are
/// cheap pointer bumps.
pub fn filter_transcript(messages: &[Arc<Message>]) -> Vec<Arc<Message>> {
    let mut filtered: Vec<Arc<Message>> = Vec::with_capacity(messages.len());

    for arc in messages {
        if let Message::Assistant(a) = arc.as_ref()
            && let LlmMessage::Assistant { content, .. } = &a.message
        {
            if is_whitespace_only_assistant(content) {
                continue;
            }
            if is_thinking_only_assistant(content) {
                continue;
            }
        }
        filtered.push(arc.clone());
    }

    strip_unresolved_tool_uses(&mut filtered);
    filtered
}

/// Assistant content is "whitespace-only" when it is non-empty and every
/// part is a Text block whose `.trim()` is empty. ANY non-text block
/// (Reasoning / ReasoningFile / ToolCall / File / Source / …) makes the
/// message substantive and keeps it — mirrors
/// `hasOnlyWhitespaceTextContent` (`utils/messages.ts:4835`), which
/// returns `false` for any non-text block AND for empty content
/// (`length === 0`). A message like `[Text("  "), Reasoning(..)]` (model
/// emitted leading whitespace before a thinking block, then the turn was
/// cancelled) therefore survives here; pure-reasoning turns
/// (no Text) are dropped by [`is_thinking_only_assistant`] instead.
fn is_whitespace_only_assistant(content: &[AssistantContentPart]) -> bool {
    !content.is_empty()
        && content.iter().all(|part| match part {
            AssistantContentPart::Text(t) => t.text.trim().is_empty(),
            _ => false,
        })
}

/// Assistant content is "thinking-only" when it contains zero Text and
/// zero ToolCall parts (only Reasoning / ReasoningFile / Custom /
/// Source / etc.). Mirrors `filterOrphanedThinkingOnlyMessages`.
fn is_thinking_only_assistant(content: &[AssistantContentPart]) -> bool {
    !content.iter().any(|part| {
        matches!(
            part,
            AssistantContentPart::Text(_) | AssistantContentPart::ToolCall(_)
        )
    })
}

/// Strip trailing assistant messages whose `ToolCall`s have no
/// downstream `Message::ToolResult` to resolve them. Walks forward
/// first to collect every resolved `tool_use_id`, then pops trailing
/// assistants with orphan calls.
fn strip_unresolved_tool_uses(messages: &mut Vec<Arc<Message>>) {
    // Owned String set — pop() needs a mutable borrow on `messages`,
    // so we can't keep a borrow into the vec while mutating it.
    let resolved_ids: std::collections::HashSet<String> = messages
        .iter()
        .filter_map(|arc| match arc.as_ref() {
            Message::ToolResult(trm) => Some(trm.tool_use_id.clone()),
            _ => None,
        })
        .collect();

    while let Some(last) = messages.last() {
        let Message::Assistant(a) = last.as_ref() else {
            break;
        };
        let LlmMessage::Assistant { content, .. } = &a.message else {
            break;
        };
        let has_unresolved = content.iter().any(|part| {
            matches!(
                part,
                AssistantContentPart::ToolCall(tc) if !resolved_ids.contains(&tc.tool_call_id)
            )
        });
        if has_unresolved {
            messages.pop();
        } else {
            break;
        }
    }
}

#[cfg(test)]
#[path = "transcript.test.rs"]
mod tests;
