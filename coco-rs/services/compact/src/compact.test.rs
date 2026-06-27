use std::sync::Arc;
use std::sync::Mutex;

use coco_llm_types::AssistantContentPart;
use coco_llm_types::ToolCallPart;
use coco_messages::AssistantMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::ToolContent;
use coco_messages::ToolResultContent;
use coco_messages::ToolResultMessage;
use coco_messages::UserMessage;
use coco_types::StopReason;
use coco_types::ToolId;
use coco_types::ToolName;
use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::*;

fn make_user_text(text: &str) -> Arc<Message> {
    Arc::new(Message::User(UserMessage {
        message: LlmMessage::user_text(text),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    }))
}

fn make_assistant_text(text: &str) -> Arc<Message> {
    Arc::new(Message::Assistant(AssistantMessage {
        message: LlmMessage::assistant(vec![AssistantContentPart::Text(
            coco_llm_types::TextPart::new(text.to_string()),
        )]),
        uuid: Uuid::new_v4(),
        model: "test".to_string(),
        stop_reason: Some(StopReason::EndTurn),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    }))
}

fn make_assistant_tool_call(tool_call_id: &str) -> Arc<Message> {
    Arc::new(Message::Assistant(AssistantMessage {
        message: LlmMessage::assistant(vec![AssistantContentPart::ToolCall(ToolCallPart::new(
            tool_call_id,
            ToolName::Read.as_str(),
            serde_json::json!({"file_path": "/tmp/recent.txt"}),
        ))]),
        uuid: Uuid::new_v4(),
        model: "test".to_string(),
        stop_reason: Some(StopReason::ToolUse),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    }))
}

fn make_tool_result(tool_call_id: &str, text: &str) -> Arc<Message> {
    Arc::new(Message::ToolResult(ToolResultMessage {
        uuid: Uuid::new_v4(),
        source_assistant_uuid: None,
        display_data: None,
        message: LlmMessage::Tool {
            content: vec![ToolContent::ToolResult(ToolResultContent {
                tool_call_id: tool_call_id.to_string(),
                tool_name: ToolName::Read.as_str().to_string(),
                output: coco_llm_types::ToolResultContent::text(text.to_string()),
                is_error: false,
                provider_metadata: None,
            })],
            provider_options: None,
        },
        tool_use_id: tool_call_id.to_string(),
        tool_id: ToolId::Builtin(ToolName::Read),
        is_error: false,
    }))
}

fn messages_contain_text(messages: &[Arc<Message>], needle: &str) -> bool {
    messages
        .iter()
        .filter_map(|message| crate::summary_text::extract_message_text(message))
        .any(|text| text.contains(needle))
}

#[test]
fn test_compact_run_options_default_mirrors_ts_full_compact_no_recent_rounds() {
    assert_eq!(CompactRunOptions::default().keep_recent_rounds, 0);
}

#[tokio::test]
async fn test_full_compact_default_summarizes_recent_tool_result_without_keeping_original() {
    let messages = vec![
        make_user_text("older request"),
        make_assistant_text("older answer"),
        make_user_text("recent tool request"),
        make_assistant_tool_call("call_recent"),
        make_tool_result("call_recent", "recent tool output"),
    ];

    let captured = Arc::new(Mutex::new(None));
    let result = compact_conversation(
        &messages,
        &CompactRunOptions::default(),
        {
            let captured = Arc::clone(&captured);
            move |attempt| {
                let captured = Arc::clone(&captured);
                async move {
                    *captured.lock().expect("capture lock") = Some(attempt);
                    Ok(CompactSummaryResponse {
                        summary: "summary includes the recent tool output".to_string(),
                    })
                }
            }
        },
        None,
    )
    .await
    .expect("compact succeeds");

    let attempt = captured
        .lock()
        .expect("capture lock")
        .clone()
        .expect("summarizer attempt captured");
    assert!(
        messages_contain_text(&attempt.context_messages, "recent tool output"),
        "full compact should summarize the full conversation, including recent tool results"
    );
    assert!(
        result.messages_to_keep.is_empty(),
        "TS full compact keeps no recent original rounds"
    );

    let post_compact_messages = build_post_compact_messages(&result);
    assert!(
        !post_compact_messages
            .iter()
            .any(|message| matches!(message.as_ref(), Message::ToolResult(_))),
        "recent tool result should not be preserved as an original post-compact message"
    );
}
