use super::*;
use coco_messages::AssistantContent;
use coco_messages::AssistantMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::ReasoningContent;
use coco_messages::TextContent;
use coco_messages::ToolCallContent;
use coco_messages::UserMessage;
use coco_types::AttachmentKind;
use coco_types::AttachmentMessage;
use coco_types::ToolName;
use pretty_assertions::assert_eq;
use uuid::Uuid;

// ── Builders ──

fn assistant_text(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::Text(TextContent {
                text: text.to_string(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: String::new(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

fn assistant_tool_call(name: &str) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::ToolCall(ToolCallContent {
                tool_call_id: format!("call-{name}"),
                tool_name: name.to_string(),
                input: serde_json::json!({}),
                provider_executed: None,
                provider_metadata: None,
                invalid: false,
                invalid_reason: None,
            })],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: String::new(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

fn assistant_thinking_only() -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::Reasoning(ReasoningContent {
                text: "hmm".to_string(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: String::new(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

fn user(text: &str) -> Message {
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

fn attachment(kind: AttachmentKind) -> Message {
    Message::Attachment(AttachmentMessage::api(
        kind,
        LlmMessage::user_text("reminder"),
    ))
}

// ── count_assistant_turns_since_tool ──

#[test]
fn returns_zero_when_last_assistant_turn_invoked_the_tool() {
    let msgs = vec![
        user("hi"),
        assistant_text("hello"),
        user("what's next"),
        assistant_tool_call("TodoWrite"),
    ];
    assert_eq!(
        count_assistant_turns_since_tool(&msgs, ToolName::TodoWrite),
        0
    );
}

#[test]
fn counts_turns_between_tool_use_and_end() {
    // TodoWrite, then 3 assistant turns.
    let msgs = vec![
        assistant_tool_call("TodoWrite"),
        assistant_text("one"),
        assistant_text("two"),
        assistant_text("three"),
    ];
    assert_eq!(
        count_assistant_turns_since_tool(&msgs, ToolName::TodoWrite),
        3
    );
}

#[test]
fn thinking_messages_are_skipped_by_counter() {
    let msgs = vec![
        assistant_tool_call("TodoWrite"),
        assistant_text("one"),
        assistant_thinking_only(), // skipped
        assistant_text("two"),
    ];
    assert_eq!(
        count_assistant_turns_since_tool(&msgs, ToolName::TodoWrite),
        2
    );
}

#[test]
fn user_messages_do_not_affect_count() {
    let msgs = vec![
        assistant_tool_call("TodoWrite"),
        user("a"),
        user("b"),
        assistant_text("one"),
        user("c"),
    ];
    assert_eq!(
        count_assistant_turns_since_tool(&msgs, ToolName::TodoWrite),
        1
    );
}

#[test]
fn never_invoked_tool_returns_total_assistant_turn_count() {
    let msgs = vec![
        assistant_text("a"),
        assistant_text("b"),
        assistant_thinking_only(),
        assistant_text("c"),
    ];
    assert_eq!(
        count_assistant_turns_since_tool(&msgs, ToolName::TodoWrite),
        3
    );
}

#[test]
fn empty_history_returns_zero() {
    let msgs: Vec<Message> = vec![];
    assert_eq!(
        count_assistant_turns_since_tool(&msgs, ToolName::TodoWrite),
        0
    );
}

// ── count_assistant_turns_since_attachment ──

#[test]
fn assistant_turns_since_attachment_counts_from_marker() {
    let msgs = vec![
        attachment(AttachmentKind::TodoReminder),
        assistant_text("one"),
        assistant_thinking_only(),
        assistant_text("two"),
    ];

    assert_eq!(
        count_assistant_turns_since_attachment(&msgs, AttachmentKind::TodoReminder),
        2
    );
}

#[test]
fn assistant_turns_since_attachment_returns_sentinel_when_absent() {
    let msgs = vec![assistant_text("one")];

    assert_eq!(
        count_assistant_turns_since_attachment(&msgs, AttachmentKind::TodoReminder),
        i32::MAX
    );
}

// ── count_assistant_turns_since_any_tool ──

#[test]
fn any_tool_matches_first_seen_going_backwards() {
    // Most recent tool use is TaskUpdate; TaskCreate is older.
    let msgs = vec![
        assistant_tool_call("TaskCreate"),
        assistant_text("one"),
        assistant_tool_call("TaskUpdate"),
        assistant_text("two"),
    ];
    let tools = &[
        ToolName::TaskCreate,
        ToolName::TaskUpdate,
        ToolName::TaskStop,
    ];
    // Walking backward: two, TaskUpdate → stop. Count of assistant turns
    // before = 1 ("two").
    assert_eq!(count_assistant_turns_since_any_tool(&msgs, tools), 1);
}

#[test]
fn any_tool_returns_total_when_no_match() {
    let msgs = vec![
        assistant_text("a"),
        assistant_tool_call("Read"),
        assistant_text("b"),
    ];
    let tools = &[ToolName::TaskCreate, ToolName::TaskUpdate];
    assert_eq!(count_assistant_turns_since_any_tool(&msgs, tools), 3);
}

#[test]
fn mixed_tool_names_only_match_exact() {
    let msgs = vec![assistant_tool_call("TaskCreate"), assistant_text("after")];
    // `ToolName::TaskUpdate` is never invoked → return total assistant-turn count (2).
    assert_eq!(
        count_assistant_turns_since_tool(&msgs, ToolName::TaskUpdate),
        2
    );
}

#[test]
fn task_management_tools_constant_contains_only_mutation_tools() {
    // The silence counter only sees TaskCreate + TaskUpdate as activity.
    // Read-only tools are excluded.
    assert_eq!(
        TASK_MANAGEMENT_TOOLS,
        &[ToolName::TaskCreate, ToolName::TaskUpdate]
    );
    assert!(!TASK_MANAGEMENT_TOOLS.contains(&ToolName::TaskGet));
    assert!(!TASK_MANAGEMENT_TOOLS.contains(&ToolName::TaskList));
    assert!(!TASK_MANAGEMENT_TOOLS.contains(&ToolName::TaskStop));
    assert!(!TASK_MANAGEMENT_TOOLS.contains(&ToolName::TaskOutput));
    assert!(!TASK_MANAGEMENT_TOOLS.contains(&ToolName::TodoWrite));
    assert!(!TASK_MANAGEMENT_TOOLS.contains(&ToolName::Read));
}

// ── total_assistant_turns ──

#[test]
fn total_assistant_turns_skips_thinking_and_non_assistant() {
    let msgs = vec![
        user("a"),
        assistant_text("one"),
        assistant_thinking_only(),
        assistant_text("two"),
        user("b"),
        assistant_tool_call("Read"),
    ];
    assert_eq!(total_assistant_turns(&msgs), 3);
}

#[test]
fn total_assistant_turns_empty() {
    let empty: &[Message] = &[];
    assert_eq!(total_assistant_turns(empty), 0);
}

// ── count_human_turns ──

#[test]
fn count_human_turns_ignores_meta_attachments_and_non_user() {
    // Post-Phase-2: reminder-injected content is Message::Attachment,
    // not User{is_meta:true}. count_human_turns simply counts User.
    let meta = Message::Attachment(coco_messages::AttachmentMessage::api(
        coco_types::AttachmentKind::CriticalSystemReminder,
        LlmMessage::user_text("system-injected"),
    ));
    let msgs = vec![
        user("real 1"),
        assistant_text("reply"),
        meta, // reminder attachment — ignored by count_human_turns
        user("real 2"),
        assistant_tool_call("Read"),
    ];
    assert_eq!(count_human_turns(&msgs), 2);
}

#[test]
fn count_human_turns_empty_history_returns_zero() {
    let empty: &[Message] = &[];
    assert_eq!(count_human_turns(empty), 0);
}

// ── count_human_turns_since_attachment ──

#[test]
fn count_human_turns_since_attachment_counts_after_marker() {
    let marker = Message::Attachment(coco_messages::AttachmentMessage::api(
        AttachmentKind::PlanModeExit,
        LlmMessage::user_text("exited"),
    ));
    let msgs = vec![
        user("before"),
        marker,
        assistant_tool_call("Read"),
        user("one"),
        assistant_text("reply"),
        user("two"),
    ];

    assert_eq!(
        count_human_turns_since_attachment(&msgs, AttachmentKind::PlanModeExit),
        2
    );
}

#[test]
fn count_human_turns_since_attachment_returns_zero_at_marker() {
    let marker = Message::Attachment(coco_messages::AttachmentMessage::api(
        AttachmentKind::PlanModeExit,
        LlmMessage::user_text("exited"),
    ));
    let msgs = vec![user("before"), marker];

    assert_eq!(
        count_human_turns_since_attachment(&msgs, AttachmentKind::PlanModeExit),
        0
    );
}

#[test]
fn count_human_turns_since_attachment_returns_zero_when_missing() {
    let msgs = vec![user("one"), assistant_text("reply"), user("two")];

    assert_eq!(
        count_human_turns_since_attachment(&msgs, AttachmentKind::PlanModeExit),
        0
    );
}
