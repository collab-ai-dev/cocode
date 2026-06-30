use crate::AssistantContent;
use crate::LlmMessage;
use crate::Message;
use crate::create_meta_message;
use crate::merge_consecutive_user_messages;
use crate::normalize::filter_orphaned_thinking_only_messages;
use coco_types::ToolName;
use std::collections::HashSet;

pub const RESUME_CONTINUATION_PROMPT: &str = "Continue from where you left off.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnInterruptionState {
    None,
    InterruptedPrompt { message_uuid: uuid::Uuid },
}

#[derive(Debug, Clone)]
pub struct ResumeSanitizationResult {
    pub messages: Vec<Message>,
    pub turn_interruption_state: TurnInterruptionState,
}

pub fn sanitize_messages_for_resume(messages: Vec<Message>) -> ResumeSanitizationResult {
    let mut messages = filter_unresolved_tool_uses(messages);
    filter_orphaned_thinking_only_messages(&mut messages);
    filter_whitespace_only_assistant_messages(&mut messages);

    let internal_state = detect_turn_interruption(&messages);
    let turn_interruption_state = match internal_state {
        InternalInterruptionState::None => TurnInterruptionState::None,
        InternalInterruptionState::InterruptedPrompt { message_uuid } => {
            TurnInterruptionState::InterruptedPrompt { message_uuid }
        }
        InternalInterruptionState::InterruptedTurn => {
            let continuation = create_meta_message(RESUME_CONTINUATION_PROMPT);
            let message_uuid = continuation
                .uuid()
                .copied()
                .unwrap_or_else(uuid::Uuid::new_v4);
            messages.push(continuation);
            TurnInterruptionState::InterruptedPrompt { message_uuid }
        }
    };

    ResumeSanitizationResult {
        messages,
        turn_interruption_state,
    }
}

fn filter_unresolved_tool_uses(messages: Vec<Message>) -> Vec<Message> {
    let mut tool_use_ids = HashSet::new();
    let mut tool_result_ids = HashSet::new();
    for message in &messages {
        for id in assistant_tool_use_ids(message) {
            tool_use_ids.insert(id.to_string());
        }
        if let Message::ToolResult(result) = message {
            tool_result_ids.insert(result.tool_use_id.clone());
        }
    }

    let unresolved = tool_use_ids
        .difference(&tool_result_ids)
        .cloned()
        .collect::<HashSet<_>>();
    if unresolved.is_empty() {
        return messages;
    }

    messages
        .into_iter()
        .filter(|message| {
            let ids = assistant_tool_use_ids(message);
            ids.is_empty() || !ids.iter().all(|id| unresolved.contains(*id))
        })
        .collect()
}

fn filter_whitespace_only_assistant_messages(messages: &mut Vec<Message>) {
    let before = messages.len();
    messages.retain(|message| {
        let Message::Assistant(assistant) = message else {
            return true;
        };
        let LlmMessage::Assistant { content, .. } = &assistant.message else {
            return true;
        };
        !has_only_whitespace_text_content(content)
    });
    if messages.len() != before {
        merge_consecutive_user_messages(messages);
    }
}

fn has_only_whitespace_text_content(content: &[AssistantContent]) -> bool {
    !content.is_empty()
        && content
            .iter()
            .all(|part| matches!(part, AssistantContent::Text(text) if text.text.trim().is_empty()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InternalInterruptionState {
    None,
    InterruptedPrompt { message_uuid: uuid::Uuid },
    InterruptedTurn,
}

fn detect_turn_interruption(messages: &[Message]) -> InternalInterruptionState {
    let Some((idx, last_message)) = messages
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| is_turn_relevant_message(message))
    else {
        return InternalInterruptionState::None;
    };

    match last_message {
        Message::Assistant(_) => InternalInterruptionState::None,
        Message::User(user) => {
            if user.is_compact_summary || user.is_visible_in_transcript_only {
                return InternalInterruptionState::None;
            }
            InternalInterruptionState::InterruptedPrompt {
                message_uuid: user.uuid,
            }
        }
        Message::Attachment(attachment) => {
            if is_ambient_memory_attachment(attachment.kind) {
                InternalInterruptionState::None
            } else {
                InternalInterruptionState::InterruptedTurn
            }
        }
        Message::ToolResult(result) => {
            if is_terminal_tool_result_id(&result.tool_use_id, messages, idx) {
                InternalInterruptionState::None
            } else {
                InternalInterruptionState::InterruptedTurn
            }
        }
        Message::System(_) | Message::Progress(_) | Message::Tombstone(_) => {
            InternalInterruptionState::None
        }
    }
}

fn is_turn_relevant_message(message: &Message) -> bool {
    !matches!(message, Message::System(_) | Message::Progress(_))
        && !matches!(message, Message::Attachment(attachment) if is_ambient_memory_attachment(attachment.kind))
        && !matches!(message, Message::Assistant(assistant) if assistant.api_error.is_some())
}

fn is_ambient_memory_attachment(kind: coco_types::AttachmentKind) -> bool {
    matches!(
        kind,
        coco_types::AttachmentKind::MemoryIndexWarning
            | coco_types::AttachmentKind::MemoryUpdateReminder
    )
}

fn assistant_tool_use_ids(message: &Message) -> Vec<&str> {
    let Message::Assistant(assistant) = message else {
        return Vec::new();
    };
    let LlmMessage::Assistant { content, .. } = &assistant.message else {
        return Vec::new();
    };
    content
        .iter()
        .filter_map(|part| match part {
            AssistantContent::ToolCall(call) => Some(call.tool_call_id.as_str()),
            _ => None,
        })
        .collect()
}

fn is_terminal_tool_result_id(tool_use_id: &str, messages: &[Message], result_idx: usize) -> bool {
    for message in messages[..result_idx].iter().rev() {
        let Message::Assistant(assistant) = message else {
            continue;
        };
        let LlmMessage::Assistant { content, .. } = &assistant.message else {
            continue;
        };
        for part in content {
            let AssistantContent::ToolCall(call) = part else {
                continue;
            };
            if call.tool_call_id == tool_use_id {
                return call.tool_name == ToolName::SendUserMessage.as_str()
                    || call.tool_name == "Brief";
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AssistantMessage;
    use crate::AttachmentBody;
    use crate::AttachmentMessage;
    use crate::TextContent;
    use crate::ToolCallContent;
    use crate::UserMessage;
    use coco_types::AttachmentKind;
    use coco_types::ToolId;
    use uuid::Uuid;

    fn user_text(text: &str) -> Message {
        Message::User(UserMessage {
            message: LlmMessage::user_text(text),
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            is_visible_in_transcript_only: false,
            is_virtual: false,
            is_compact_summary: false,
            permission_mode: None,
            origin: None,
            parent_tool_use_id: None,
        })
    }

    fn assistant_with(content: Vec<AssistantContent>, request_id: Option<&str>) -> Message {
        Message::Assistant(AssistantMessage {
            message: LlmMessage::assistant(content),
            uuid: Uuid::new_v4(),
            model: "test".into(),
            stop_reason: None,
            usage: None,
            cost_usd: None,
            request_id: request_id.map(str::to_string),
            api_error: None,
        })
    }

    fn assistant_text(text: &str) -> Message {
        assistant_with(
            vec![AssistantContent::Text(TextContent {
                text: text.into(),
                provider_metadata: None,
            })],
            None,
        )
    }

    fn assistant_tool(tool_use_id: &str, tool_name: &str) -> Message {
        assistant_with(
            vec![AssistantContent::ToolCall(ToolCallContent::new(
                tool_use_id.to_string(),
                tool_name.to_string(),
                serde_json::json!({}),
            ))],
            None,
        )
    }

    fn tool_result(tool_use_id: &str, tool_name: &str) -> Message {
        crate::create_tool_result_message(
            tool_use_id,
            tool_name,
            ToolId::Custom(tool_name.into()),
            "ok",
            false,
        )
    }

    fn reminder_attachment(kind: AttachmentKind) -> Message {
        Message::Attachment(AttachmentMessage::api(
            kind,
            LlmMessage::user_text(crate::wrapping::wrap_in_system_reminder("notice")),
        ))
    }

    fn has_visible_no_response_sentinel(messages: &[Message]) -> bool {
        messages.iter().any(|message| {
            let Message::Assistant(assistant) = message else {
                return false;
            };
            let LlmMessage::Assistant { content, .. } = &assistant.message else {
                return false;
            };
            content.iter().any(|part| {
                matches!(part, AssistantContent::Text(text) if text.text == "No response requested.")
            })
        })
    }

    #[test]
    fn drops_whitespace_only_assistant_messages() {
        let result = sanitize_messages_for_resume(vec![
            user_text("hi"),
            assistant_text("\n \t"),
            user_text("again"),
        ]);

        assert_eq!(result.messages.len(), 1);
        assert!(matches!(result.messages[0], Message::User(_)));
    }

    #[test]
    fn drops_orphaned_thinking_only_assistant_messages() {
        let result = sanitize_messages_for_resume(vec![assistant_with(
            vec![AssistantContent::Reasoning(coco_llm_types::ReasoningPart {
                text: "thinking".into(),
                provider_metadata: None,
            })],
            None,
        )]);

        assert!(result.messages.is_empty());
        assert_eq!(result.turn_interruption_state, TurnInterruptionState::None);
    }

    #[test]
    fn drops_unresolved_trailing_tool_use_and_marks_interrupted_prompt() {
        let user = user_text("run tool");
        let user_uuid = user.uuid().copied().expect("user uuid");
        let result = sanitize_messages_for_resume(vec![user, assistant_tool("toolu_1", "Read")]);

        assert_eq!(result.messages.len(), 1);
        assert_eq!(
            result.turn_interruption_state,
            TurnInterruptionState::InterruptedPrompt {
                message_uuid: user_uuid,
            }
        );
    }

    #[test]
    fn trailing_tool_result_gets_hidden_continuation_without_visible_sentinel() {
        let result = sanitize_messages_for_resume(vec![
            user_text("run tool"),
            assistant_tool("toolu_1", "Read"),
            tool_result("toolu_1", "Read"),
        ]);

        assert!(matches!(
            result.turn_interruption_state,
            TurnInterruptionState::InterruptedPrompt { .. }
        ));
        assert_eq!(
            result.messages.last().and_then(|message| match message {
                Message::Attachment(attachment) => match &attachment.body {
                    AttachmentBody::Api(LlmMessage::User { content, .. }) => {
                        content.first().and_then(|part| match part {
                            crate::UserContent::Text(text) => Some(text.text.as_str()),
                            _ => None,
                        })
                    }
                    _ => None,
                },
                _ => None,
            }),
            Some(RESUME_CONTINUATION_PROMPT)
        );
        assert!(!has_visible_no_response_sentinel(&result.messages));
    }

    #[test]
    fn trailing_memory_update_attachment_does_not_mark_interrupted_turn() {
        let result = sanitize_messages_for_resume(vec![
            user_text("done"),
            assistant_text("finished"),
            reminder_attachment(AttachmentKind::MemoryUpdateReminder),
        ]);

        assert_eq!(result.turn_interruption_state, TurnInterruptionState::None);
        assert_eq!(result.messages.len(), 3);
    }

    #[test]
    fn trailing_memory_warning_attachment_does_not_hide_interrupted_prompt() {
        let user = user_text("pending");
        let user_uuid = user.uuid().copied().expect("user uuid");
        let result = sanitize_messages_for_resume(vec![
            user,
            reminder_attachment(AttachmentKind::MemoryIndexWarning),
        ]);

        assert_eq!(
            result.turn_interruption_state,
            TurnInterruptionState::InterruptedPrompt {
                message_uuid: user_uuid,
            }
        );
    }

    #[test]
    fn trailing_non_memory_attachment_still_marks_interrupted_turn() {
        let result = sanitize_messages_for_resume(vec![
            user_text("done"),
            assistant_text("finished"),
            reminder_attachment(AttachmentKind::CriticalSystemReminder),
        ]);

        assert!(matches!(
            result.turn_interruption_state,
            TurnInterruptionState::InterruptedPrompt { .. }
        ));
        assert_eq!(result.messages.len(), 4);
    }

    #[test]
    fn send_user_message_tool_result_is_terminal() {
        let result = sanitize_messages_for_resume(vec![
            user_text("brief"),
            assistant_tool("toolu_1", ToolName::SendUserMessage.as_str()),
            tool_result("toolu_1", ToolName::SendUserMessage.as_str()),
        ]);

        assert_eq!(result.turn_interruption_state, TurnInterruptionState::None);
        assert_eq!(result.messages.len(), 3);
    }
}
