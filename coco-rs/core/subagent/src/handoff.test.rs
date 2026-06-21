use super::*;

#[test]
fn should_classify_gates_only_on_nonempty_transcript() {
    // Read-only agents and zero-tool turns are NOT exempt —
    // classification runs whenever the transcript is non-empty.
    assert!(!should_classify(""));
    assert!(!should_classify("   \n  "));
    assert!(should_classify("[user] hello"));
}

#[test]
fn stage1_prompts_include_handoff_review_text_and_metadata() {
    let (sys, user) = stage1_prompts("worker", "[user] hello\n[assistant] hi", 3);
    assert!(sys.contains("hand-off"));
    // Hand-off review framing must appear verbatim — the classifier's
    // training surface anchors on this exact phrasing
    // (`agentToolUtils.ts:417`).
    assert!(user.contains(HANDOFF_REVIEW_USER_PROMPT));
    assert!(user.contains("Sub-agent type: worker"));
    assert!(user.contains("Tool uses: 3"));
    assert!(user.contains("[user] hello"));
    assert!(user.contains("`SAFE`"));
}

#[test]
fn stage2_prompts_carry_stage1_verdict_and_review_framing() {
    let (sys, user) = stage2_prompts("BLOCKED: rm -rf /", "transcript body");
    assert!(sys.contains("second-stage"));
    assert!(user.contains(HANDOFF_REVIEW_USER_PROMPT));
    assert!(user.contains("BLOCKED: rm -rf /"));
    assert!(user.contains("transcript body"));
}

#[test]
fn handoff_classifier_active_requires_auto_mode_and_feature() {
    use coco_types::PermissionMode;
    assert!(handoff_classifier_active(Some(PermissionMode::Auto), true));
    assert!(!handoff_classifier_active(
        Some(PermissionMode::Auto),
        false
    ));
    assert!(!handoff_classifier_active(
        Some(PermissionMode::AcceptEdits),
        true
    ));
    assert!(!handoff_classifier_active(None, true));
}

#[test]
fn parse_safe_response_returns_safe() {
    assert_eq!(
        parse_classifier_response("SAFE"),
        HandoffClassification::Safe
    );
    assert_eq!(
        parse_classifier_response("  SAFE — looks clean"),
        HandoffClassification::Safe
    );
    assert_eq!(
        parse_classifier_response("safe"),
        HandoffClassification::Safe
    );
}

#[test]
fn parse_empty_response_fails_open_to_safe() {
    assert_eq!(
        parse_classifier_response("   "),
        HandoffClassification::Safe
    );
}

#[test]
fn parse_blocked_response_strips_prefix() {
    let v = parse_classifier_response("BLOCKED: ran a destructive shell command");
    match v {
        HandoffClassification::Blocked { reason } => {
            assert_eq!(reason, "ran a destructive shell command");
        }
        _ => panic!("expected Blocked"),
    }
}

#[test]
fn parse_unmatched_response_treats_as_blocked() {
    let v = parse_classifier_response("the agent did something weird");
    matches!(v, HandoffClassification::Blocked { .. });
}

#[test]
fn render_block_message_uses_ts_warning_format() {
    let m = render_block_message(&HandoffClassification::Blocked {
        reason: "deleted prod credentials".to_string(),
    });
    let msg = m.expect("Some for blocked");
    // Prefix from `agentToolUtils.ts:476`.
    assert!(msg.starts_with("SECURITY WARNING:"));
    assert!(msg.contains("violate security policy"));
    assert!(msg.contains("Reason: deleted prod credentials"));
    assert!(msg.contains("Review the sub-agent's actions carefully"));
}

#[test]
fn render_block_message_returns_none_for_safe() {
    assert!(render_block_message(&HandoffClassification::Safe).is_none());
}

#[test]
fn render_block_message_handles_empty_reason() {
    // Classifier returning bare "BLOCKED" parses to `reason = ""`. The
    // payload still has to be a self-contained sentence — collapse the
    // empty reason to "unspecified safety concern" so neither the
    // `Reason: .` nor the trailing review hint reads broken.
    let bare_blocked = parse_classifier_response("BLOCKED");
    let msg = render_block_message(&bare_blocked).expect("Some for blocked");
    assert!(msg.contains("unspecified safety concern"));
    assert!(!msg.contains("Reason: ."), "got: {msg}");
}

#[test]
fn build_transcript_summary_compresses_tool_blocks() {
    use coco_llm_types::AssistantContentPart;
    use coco_llm_types::LlmMessage;
    use coco_llm_types::ToolCallPart;
    use coco_llm_types::ToolContentPart;
    use coco_llm_types::ToolResultContent;
    use coco_llm_types::ToolResultPart;
    use coco_types::messages::{AssistantMessage, Message, ToolResultMessage, UserMessage};
    use std::sync::Arc;
    use uuid::Uuid;

    let messages: Vec<Arc<Message>> = vec![
        Arc::new(Message::User(UserMessage {
            message: LlmMessage::user_text("do the thing"),
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            is_visible_in_transcript_only: false,
            is_virtual: false,
            is_compact_summary: false,
            permission_mode: None,
            origin: None,
            parent_tool_use_id: None,
        })),
        Arc::new(Message::Assistant(AssistantMessage {
            message: LlmMessage::Assistant {
                content: vec![
                    AssistantContentPart::text("Let me search"),
                    AssistantContentPart::ToolCall(ToolCallPart::new(
                        "tu_1",
                        "Grep",
                        serde_json::Value::Null,
                    )),
                ],
                provider_options: None,
            },
            uuid: Uuid::new_v4(),
            model: String::new(),
            stop_reason: None,
            usage: None,
            cost_usd: None,
            request_id: None,
            api_error: None,
        })),
        Arc::new(Message::ToolResult(ToolResultMessage {
            uuid: Uuid::new_v4(),
            source_assistant_uuid: None,
            display_data: None,
            message: LlmMessage::Tool {
                content: vec![ToolContentPart::ToolResult(ToolResultPart::new(
                    "tu_1",
                    "Grep",
                    ToolResultContent::text("file1.rs\nfile2.rs"),
                ))],
                provider_options: None,
            },
            tool_use_id: "tu_1".into(),
            tool_id: "Grep".parse().unwrap(),
            is_error: false,
        })),
    ];
    let summary = build_transcript_summary(&messages);
    assert!(summary.contains("[user] do the thing"));
    assert!(summary.contains("[assistant] Let me search"));
    assert!(summary.contains("[assistant] tool_use: Grep"));
    assert!(summary.contains("[user] tool_result"));
    // Tool result body NOT included.
    assert!(!summary.contains("file1.rs"));
}
