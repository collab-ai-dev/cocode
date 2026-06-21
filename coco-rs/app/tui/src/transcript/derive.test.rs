use std::sync::Arc;

use coco_messages::AssistantContent;
use coco_messages::AttachmentMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::ReasoningContent;
use coco_messages::TextContent;
use coco_messages::ToolCallContent;
use coco_types::AttachmentKind;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::message_to_cells;
use crate::transcript::cells::CellKind;

#[test]
fn ui_hidden_attachment_derives_no_cells() {
    let msg = Message::Attachment(AttachmentMessage::api(
        AttachmentKind::DeferredToolsDelta,
        LlmMessage::user_text("deferred tool reminder"),
    ));

    assert!(message_to_cells(Arc::new(msg)).is_empty());
}

#[test]
fn adjacent_reasoning_parts_coalesce_into_one_thinking_cell() {
    let msg = coco_messages::create_assistant_message(
        vec![
            AssistantContent::Reasoning(ReasoningContent::new("first")),
            AssistantContent::Reasoning(ReasoningContent::new("second")),
            AssistantContent::Reasoning(ReasoningContent::new("third")),
            AssistantContent::Text(TextContent::new("answer")),
        ],
        "test-model",
        coco_types::TokenUsage::default(),
    );

    let cells = message_to_cells(Arc::new(msg));

    assert_eq!(cells.len(), 2);
    match &cells[0].kind {
        CellKind::AssistantThinking {
            text,
            metadata_anchor,
        } => {
            assert_eq!(text, "first\n\nsecond\n\nthird");
            assert!(*metadata_anchor);
        }
        other => panic!("expected thinking cell, got {other:?}"),
    }
    match &cells[1].kind {
        CellKind::AssistantText { text, .. } => assert_eq!(text, "answer"),
        other => panic!("expected text cell, got {other:?}"),
    }
}

#[test]
fn ignored_assistant_parts_do_not_split_reasoning_runs() {
    let msg = coco_messages::create_assistant_message(
        vec![
            AssistantContent::Reasoning(ReasoningContent::new("first")),
            AssistantContent::Text(TextContent::new("")),
            AssistantContent::Reasoning(ReasoningContent::new("second")),
        ],
        "test-model",
        coco_types::TokenUsage::default(),
    );

    let cells = message_to_cells(Arc::new(msg));

    assert_eq!(cells.len(), 1);
    match &cells[0].kind {
        CellKind::AssistantThinking {
            text,
            metadata_anchor,
        } => {
            assert_eq!(text, "first\n\nsecond");
            assert!(*metadata_anchor);
        }
        other => panic!("expected coalesced thinking cell, got {other:?}"),
    }
}

#[test]
fn interleaved_reasoning_runs_stay_separate_with_one_metadata_anchor() {
    let msg = coco_messages::create_assistant_message(
        vec![
            AssistantContent::Reasoning(ReasoningContent::new("first")),
            AssistantContent::Text(TextContent::new("answer")),
            AssistantContent::Reasoning(ReasoningContent::new("second")),
            AssistantContent::ToolCall(ToolCallContent::new("call-1", "Read", json!({}))),
        ],
        "test-model",
        coco_types::TokenUsage::default(),
    );

    let cells = message_to_cells(Arc::new(msg));

    assert_eq!(cells.len(), 4);
    match &cells[0].kind {
        CellKind::AssistantThinking {
            text,
            metadata_anchor,
        } => {
            assert_eq!(text, "first");
            assert!(*metadata_anchor);
        }
        other => panic!("expected first thinking cell, got {other:?}"),
    }
    assert!(matches!(cells[1].kind, CellKind::AssistantText { .. }));
    match &cells[2].kind {
        CellKind::AssistantThinking {
            text,
            metadata_anchor,
        } => {
            assert_eq!(text, "second");
            assert!(!*metadata_anchor);
        }
        other => panic!("expected second thinking cell, got {other:?}"),
    }
    assert!(matches!(cells[3].kind, CellKind::ToolUse { .. }));
}

#[test]
fn skill_listing_attachment_derives_no_cells() {
    let msg = Message::Attachment(AttachmentMessage::api(
        AttachmentKind::SkillListing,
        LlmMessage::user_text("- review: test skill"),
    ));

    assert!(message_to_cells(Arc::new(msg)).is_empty());
}

#[test]
fn overlay_driven_tool_calls_emit_no_tool_use_cell() {
    // ExitPlanMode / AskUserQuestion render their own surface; their
    // `● ToolName(…)` invocation header must not be derived — only the result
    // renders. Bash is the control: a normal tool still yields a ToolUse cell.
    let overlay = coco_messages::create_assistant_message(
        vec![AssistantContent::ToolCall(ToolCallContent::new(
            "c1",
            "ExitPlanMode",
            json!({"plan": "# Goal"}),
        ))],
        "test-model",
        coco_types::TokenUsage::default(),
    );
    let cells = message_to_cells(Arc::new(overlay));
    assert!(
        !cells
            .iter()
            .any(|c| matches!(c.kind, CellKind::ToolUse { .. })),
        "overlay-driven tool must not derive a ToolUse cell: {cells:?}"
    );

    let normal = coco_messages::create_assistant_message(
        vec![AssistantContent::ToolCall(ToolCallContent::new(
            "c2",
            "Bash",
            json!({"command": "ls"}),
        ))],
        "test-model",
        coco_types::TokenUsage::default(),
    );
    let cells = message_to_cells(Arc::new(normal));
    assert!(
        cells
            .iter()
            .any(|c| matches!(c.kind, CellKind::ToolUse { .. })),
        "normal tool must derive a ToolUse cell: {cells:?}"
    );
}
