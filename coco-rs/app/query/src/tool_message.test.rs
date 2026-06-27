//! Tests for [`ToolMessageBuckets::flatten`] — locks in the I5 bucket
//! ordering independent of the runner / scheduler plumbing.
//!
//! The test fixtures use tagged `ToolResult` outputs so the flatten
//! order is verifiable by inspecting the message sequence alone.

use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::ToolContent;
use coco_messages::ToolResultMessage;
use coco_messages::create_user_message;
use coco_types::ToolId;
use coco_types::ToolName;
use pretty_assertions::assert_eq;
// The inner "content shape" of `ToolResultPart.output` — a separate enum
// from the outer `ToolResultContent` alias (which is `ToolResultPart`).
use coco_llm_types::ToolResultContent as InnerToolResultContent;

use super::*;

// ── Fixture helpers ───────────────────────────────────────────────

/// Tag a user-text attachment with a marker string so the flatten
/// order is trivial to assert via string comparison.
fn user_marker(text: &str) -> Message {
    // `create_user_message` already returns a `Message`.
    create_user_message(text)
}

fn tool_result_marker(tool_use_id: &str, output: &str, is_error: bool) -> Message {
    Message::ToolResult(ToolResultMessage {
        uuid: uuid::Uuid::new_v4(),
        source_assistant_uuid: None,
        display_data: None,
        message: LlmMessage::Tool {
            content: vec![ToolContent::ToolResult(coco_llm_types::ToolResultPart {
                tool_call_id: tool_use_id.into(),
                tool_name: "Read".into(),
                output: InnerToolResultContent::text(output),
                is_error,
                provider_metadata: None,
            })],
            provider_options: None,
        },
        tool_use_id: tool_use_id.into(),
        tool_id: ToolId::Builtin(ToolName::Read),
        is_error,
    })
}

/// Extract a short marker string from a Message so tests can assert
/// ordering without matching on full message structure.
fn marker_of(msg: &Message) -> String {
    match msg {
        Message::User(u) => match &u.message {
            LlmMessage::User { content, .. } => content
                .iter()
                .find_map(|p| match p {
                    coco_llm_types::UserContentPart::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
            _ => String::new(),
        },
        Message::ToolResult(tr) => match &tr.message {
            LlmMessage::Tool { content, .. } => content
                .iter()
                .find_map(|c| match c {
                    ToolContent::ToolResult(r) => Some(match &r.output {
                        InnerToolResultContent::Text { value, .. } => {
                            format!("tool_result:{value}")
                        }
                        _ => "tool_result:<other>".into(),
                    }),
                    _ => None,
                })
                .unwrap_or_default(),
            _ => String::new(),
        },
        Message::Attachment(a) => format!("attachment:{:?}", a.kind),
        other => format!("other:{other:?}"),
    }
}

fn markers(msgs: &[Message]) -> Vec<String> {
    msgs.iter().map(marker_of).collect()
}

// ── Success / non-MCP ─────────────────────────────────────────────

#[test]
fn test_success_non_mcp_order_pre_result_post_new_prevent() {
    let buckets = ToolMessageBuckets {
        pre_hook: vec![user_marker("pre-1"), user_marker("pre-2")],
        tool_result: Some(tool_result_marker("tu-1", "ok", false)),
        new_messages: vec![user_marker("new-1")],
        post_hook: vec![user_marker("post-1")],
        prevent_continuation_attachment: Some(user_marker("prevent-1")),
        path: ToolMessagePath::Success,
    };
    let out = buckets.flatten(ToolMessageOrder::NonMcp);
    assert_eq!(
        markers(&out),
        vec![
            "pre-1",
            "pre-2",
            "tool_result:ok",
            "post-1",
            "new-1",
            "prevent-1",
        ]
    );
}

#[test]
fn test_success_non_mcp_without_hooks_is_just_result_then_new() {
    let buckets = ToolMessageBuckets {
        pre_hook: vec![],
        tool_result: Some(tool_result_marker("tu-1", "ok", false)),
        new_messages: vec![user_marker("new-1"), user_marker("new-2")],
        post_hook: vec![],
        prevent_continuation_attachment: None,
        path: ToolMessagePath::Success,
    };
    let out = buckets.flatten(ToolMessageOrder::NonMcp);
    assert_eq!(markers(&out), vec!["tool_result:ok", "new-1", "new-2"]);
}

// ── Success / MCP ─────────────────────────────────────────────────

#[test]
fn test_success_mcp_order_pre_result_new_prevent_post() {
    let buckets = ToolMessageBuckets {
        pre_hook: vec![user_marker("pre-1")],
        tool_result: Some(tool_result_marker("tu-1", "mcp-ok", false)),
        new_messages: vec![user_marker("new-1")],
        post_hook: vec![user_marker("post-1"), user_marker("post-2")],
        prevent_continuation_attachment: Some(user_marker("prevent-1")),
        path: ToolMessagePath::Success,
    };
    let out = buckets.flatten(ToolMessageOrder::Mcp);
    // MCP defers post_hook to AFTER new_messages + prevent.
    assert_eq!(
        markers(&out),
        vec![
            "pre-1",
            "tool_result:mcp-ok",
            "new-1",
            "prevent-1",
            "post-1",
            "post-2",
        ]
    );
}

#[test]
fn test_success_mcp_without_prevent_still_defers_post_hook() {
    let buckets = ToolMessageBuckets {
        pre_hook: vec![],
        tool_result: Some(tool_result_marker("tu-1", "mcp-ok", false)),
        new_messages: vec![user_marker("new-1")],
        post_hook: vec![user_marker("post-1")],
        prevent_continuation_attachment: None,
        path: ToolMessagePath::Success,
    };
    let out = buckets.flatten(ToolMessageOrder::Mcp);
    assert_eq!(markers(&out), vec!["tool_result:mcp-ok", "new-1", "post-1"]);
}

// ── Failure ────────────────────────────────────────────────────────

#[test]
fn test_failure_order_pre_result_posthook_failure_only() {
    let buckets = ToolMessageBuckets {
        pre_hook: vec![user_marker("pre-1")],
        tool_result: Some(tool_result_marker("tu-1", "boom", true)),
        new_messages: vec![], // failure path never emits tool.new_messages
        post_hook: vec![user_marker("failure-hook-1")],
        prevent_continuation_attachment: None, // Success-block prevent is bypassed
        path: ToolMessagePath::Failure,
    };
    let out = buckets.flatten(ToolMessageOrder::NonMcp);
    assert_eq!(
        markers(&out),
        vec!["pre-1", "tool_result:boom", "failure-hook-1"]
    );
}

#[test]
fn test_failure_order_ignores_mcp_defer() {
    // MCP defer does NOT apply to failure path — the flatten template
    // stays pre → result → post regardless of order.
    let buckets = ToolMessageBuckets {
        pre_hook: vec![],
        tool_result: Some(tool_result_marker("tu-1", "boom", true)),
        new_messages: vec![],
        post_hook: vec![user_marker("failure-hook-1")],
        prevent_continuation_attachment: None,
        path: ToolMessagePath::Failure,
    };
    let out = buckets.flatten(ToolMessageOrder::Mcp);
    assert_eq!(markers(&out), vec!["tool_result:boom", "failure-hook-1"]);
}

// ── EarlyReturn ────────────────────────────────────────────────────

#[test]
fn test_early_return_order_pre_result_only() {
    let buckets = ToolMessageBuckets {
        pre_hook: vec![user_marker("pre-1")],
        tool_result: Some(tool_result_marker("tu-1", "Unknown tool: foo", true)),
        new_messages: vec![],
        post_hook: vec![],
        prevent_continuation_attachment: None,
        path: ToolMessagePath::EarlyReturn,
    };
    let out = buckets.flatten(ToolMessageOrder::NonMcp);
    assert_eq!(
        markers(&out),
        vec!["pre-1", "tool_result:Unknown tool: foo"]
    );
}

#[test]
fn test_early_return_without_pre_hooks_is_just_result() {
    let buckets = ToolMessageBuckets {
        pre_hook: vec![],
        tool_result: Some(tool_result_marker("tu-1", "Invalid input", true)),
        new_messages: vec![],
        post_hook: vec![],
        prevent_continuation_attachment: None,
        path: ToolMessagePath::EarlyReturn,
    };
    let out = buckets.flatten(ToolMessageOrder::NonMcp);
    assert_eq!(markers(&out), vec!["tool_result:Invalid input"]);
}
