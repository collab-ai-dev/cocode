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

/// THE regression test for the prompt-echo incident: a literal
/// summarizer copies the compaction request (directive envelope
/// included) into section 6. The echoed span must never reach the
/// post-compact history, while `raw_summary` keeps the echo verbatim
/// for PostCompact hooks.
#[tokio::test]
async fn test_full_compact_scrubs_echoed_directive_from_summary_message() {
    let messages = vec![
        make_user_text("real user request"),
        make_assistant_text("real answer"),
    ];

    let echoed_summary = format!(
        "<analysis>ok</analysis>\n<summary>\n1. Primary Request and Intent:\n   real user request\n\n6. All user messages:\n    - real user request\n    - {}\nCRITICAL: Respond with TEXT ONLY. Do NOT call any tools.\n{}\n\n7. Pending Tasks:\n   - follow up on retries\n</summary>",
        crate::prompt::COMPACT_DIRECTIVE_OPEN,
        crate::prompt::COMPACT_DIRECTIVE_CLOSE,
    );

    let result = compact_conversation(
        &messages,
        &CompactRunOptions::default(),
        {
            let echoed_summary = echoed_summary.clone();
            move |_attempt| {
                let echoed_summary = echoed_summary.clone();
                async move {
                    Ok(CompactSummaryResponse {
                        summary: echoed_summary,
                    })
                }
            }
        },
        None,
    )
    .await
    .expect("compact succeeds despite the echo");

    let summary_text: String = result
        .summary_messages
        .iter()
        .filter_map(crate::summary_text::extract_message_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        summary_text.contains("real user request"),
        "real section content must survive the scrub"
    );
    assert!(
        summary_text.contains("follow up on retries"),
        "sections after the echo must survive the scrub"
    );
    assert!(
        !summary_text.contains(crate::prompt::COMPACT_DIRECTIVE_OPEN),
        "directive tags must never reach post-compact history"
    );
    assert!(
        !summary_text.contains("CRITICAL: Respond with TEXT ONLY"),
        "echoed instruction text must never reach post-compact history"
    );
    assert!(
        result
            .raw_summary
            .as_deref()
            .is_some_and(|raw| raw.contains(crate::prompt::COMPACT_DIRECTIVE_OPEN)),
        "raw_summary keeps the echo verbatim for PostCompact hooks"
    );
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

fn make_assistant_with_media() -> Arc<Message> {
    Arc::new(Message::Assistant(AssistantMessage {
        message: LlmMessage::assistant(vec![
            AssistantContentPart::Text(coco_llm_types::TextPart::new(
                "here is the generated chart".to_string(),
            )),
            AssistantContentPart::File(coco_llm_types::FilePart::from_bytes(
                vec![0u8; 64],
                "image/png",
            )),
            AssistantContentPart::ReasoningFile(coco_llm_types::ReasoningFilePart::new(
                coco_llm_types::SharedV4FileData::data_bytes(vec![0u8; 64]),
                "application/pdf",
            )),
        ]),
        uuid: Uuid::new_v4(),
        model: "test".to_string(),
        stop_reason: Some(StopReason::EndTurn),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    }))
}

#[test]
fn test_strip_images_covers_assistant_media_parts() {
    let owned: Vec<Message> = vec![
        make_user_text("show me a chart").as_ref().clone(),
        make_assistant_with_media().as_ref().clone(),
    ];
    let stripped = strip_images_from_messages(&owned);

    let Message::Assistant(a) = &stripped[1] else {
        panic!("assistant message expected");
    };
    let LlmMessage::Assistant { content, .. } = &a.message else {
        panic!("assistant llm message expected");
    };
    let texts: Vec<&str> = content
        .iter()
        .filter_map(|p| match p {
            AssistantContentPart::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        vec!["here is the generated chart", "[image]", "[document]"],
        "File → [image], ReasoningFile → [document]; text untouched"
    );
    assert!(
        !content.iter().any(|p| matches!(
            p,
            AssistantContentPart::File(_) | AssistantContentPart::ReasoningFile(_)
        )),
        "no media parts survive"
    );
}

#[test]
fn test_strip_images_assistant_without_media_untouched() {
    // Fast path: a media-free assistant message must not be rewritten.
    assert!(strip_one_message_for_media_if_needed(&make_assistant_text("plain answer")).is_none());
}

#[test]
fn test_bound_summary_input_noop_when_fits() {
    let messages = vec![
        make_user_text("short request"),
        make_assistant_text("short answer"),
    ];
    let bounded = bound_summary_input_to_window(
        messages.clone(),
        "summarize",
        /*max_summary_tokens*/ 1_000,
        /*context_window*/ 200_000,
    );
    assert_eq!(bounded.len(), messages.len());
}

#[test]
fn test_bound_summary_input_drops_head_rounds_when_over_window() {
    // 10 rounds of ~8 KB each (~2k estimated tokens per round). Window
    // 30k → budget = 27k − prompt − 20k output reserve ≈ 7k tokens, so
    // most head rounds must be dropped before the first summarizer call.
    let big = "x".repeat(4_000);
    let mut messages = Vec::new();
    for i in 0..10 {
        messages.push(make_user_text(&format!("round {i} {big}")));
        messages.push(make_assistant_text(&format!("answer {i} {big}")));
    }
    let bounded = bound_summary_input_to_window(
        messages.clone(),
        "summarize",
        MAX_OUTPUT_TOKENS_FOR_SUMMARY,
        /*context_window*/ 30_000,
    );
    assert!(
        bounded.len() < messages.len(),
        "head rounds must be dropped proactively"
    );
    // The most recent round always survives.
    assert!(messages_contain_text(&bounded, "answer 9"));
    // And the estimate now fits the derived budget.
    let budget = (30_000f64 * SUMMARY_INPUT_WINDOW_FILL_RATIO) as i64
        - coco_messages::estimate_text_tokens("summarize")
        - MAX_OUTPUT_TOKENS_FOR_SUMMARY;
    assert!(coco_messages::estimate_tokens_for_messages(&bounded) <= budget);
}

#[test]
fn test_bound_summary_input_degenerate_budget_is_noop() {
    // Window smaller than the output reservation → budget ≤ 0: leave the
    // input alone and let the reactive PTL retry own recovery.
    let messages = vec![
        make_user_text("request"),
        make_assistant_text("answer"),
        make_user_text("more"),
        make_assistant_text("more answer"),
    ];
    let bounded = bound_summary_input_to_window(
        messages.clone(),
        "summarize",
        MAX_OUTPUT_TOKENS_FOR_SUMMARY,
        /*context_window*/ 1_000,
    );
    assert_eq!(bounded.len(), messages.len());
}

#[tokio::test]
async fn test_full_compact_bounds_summarizer_input_before_first_call() {
    let big = "x".repeat(4_000);
    let mut messages = Vec::new();
    for i in 0..10 {
        messages.push(make_user_text(&format!("round {i} {big}")));
        messages.push(make_assistant_text(&format!("answer {i} {big}")));
    }

    let attempt_sizes: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let config = CompactRunOptions {
        context_window: 30_000,
        ..Default::default()
    };
    compact_conversation(
        &messages,
        &config,
        {
            let attempt_sizes = Arc::clone(&attempt_sizes);
            move |attempt| {
                let attempt_sizes = Arc::clone(&attempt_sizes);
                async move {
                    attempt_sizes
                        .lock()
                        .expect("capture lock")
                        .push(attempt.messages.len());
                    Ok(CompactSummaryResponse {
                        summary: "<summary>ok</summary>".to_string(),
                    })
                }
            }
        },
        None,
    )
    .await
    .expect("compact succeeds");

    let sizes = attempt_sizes.lock().expect("capture lock");
    assert_eq!(sizes.len(), 1, "no reactive PTL retries were needed");
    assert!(
        sizes[0] < messages.len(),
        "the first summarizer call already received a bounded slice"
    );
}
