use super::*;
use crate::ToolResultContent;

#[test]
fn test_estimate_text_tokens() {
    // 100 chars ≈ 25 tokens (at 4 chars/token)
    assert_eq!(estimate_text_tokens("a".repeat(100).as_str()), 25);
    assert_eq!(estimate_text_tokens(""), 0);
}

#[test]
fn test_is_over_threshold() {
    assert!(is_over_threshold(95_000, 100_000, 90));
    assert!(!is_over_threshold(85_000, 100_000, 90));
    assert!(!is_over_threshold(100, 0, 90)); // zero context window
}

#[test]
fn test_transcript_only_user_costs_zero_tokens() {
    use coco_types::messages::Message;
    use coco_types::messages::UserMessage;
    let body = "x".repeat(400); // ~100 tokens if charged
    let mk = |transcript_only: bool| {
        Message::User(UserMessage {
            message: coco_types::messages::LlmMessage::user_text(&body),
            uuid: uuid::Uuid::new_v4(),
            timestamp: String::new(),
            is_visible_in_transcript_only: transcript_only,
            is_virtual: false,
            is_compact_summary: false,
            permission_mode: None,
            origin: None,
            parent_tool_use_id: None,
        })
    };
    // Sent to the model → charged; transcript-only → never sent → 0.
    assert!(estimate_message_tokens(&mk(false)) > 0);
    assert_eq!(estimate_message_tokens(&mk(true)), 0);
}

#[test]
fn test_multipart_tiny_tool_result_charges_every_part() {
    let part_count = 100;
    let prompt = vec![LlmMessage::Tool {
        content: vec![ToolContent::ToolResult(ToolResultContent {
            tool_call_id: "call".to_string(),
            tool_name: "McpTool".to_string(),
            output: crate::ToolResultOutput::Content {
                value: (0..part_count)
                    .map(|_| crate::ToolResultContentPart::text("x"))
                    .collect(),
                provider_options: None,
            },
            is_error: false,
            provider_metadata: None,
        })],
        provider_options: None,
    }];

    assert!(estimate_tokens_for_prompt(&prompt) >= i64::from(part_count));
}
