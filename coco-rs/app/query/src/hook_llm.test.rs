use super::*;
use coco_inference::ResponseFormat;
use coco_llm_types::TextPart;
use serde_json::json;

#[test]
fn test_parse_hook_response_ok_true() {
    let content = vec![AssistantContentPart::Text(TextPart::new(r#"{"ok": true}"#))];
    let result = parse_hook_response(&content, coco_types::HookEventType::PreToolUse);
    assert!(matches!(
        result,
        HookEvaluationResult::Success { reason: None }
    ));
}

#[test]
fn test_parse_hook_response_ok_false_with_reason() {
    let content = vec![AssistantContentPart::Text(TextPart::new(
        r#"{"ok": false, "reason": "found AWS key"}"#,
    ))];
    let result = parse_hook_response(&content, coco_types::HookEventType::PreToolUse);
    match result {
        HookEvaluationResult::Blocking { reason } => assert_eq!(reason, "found AWS key"),
        other => panic!("expected Blocking, got {other:?}"),
    }
}

#[test]
fn test_parse_hook_response_ok_false_missing_reason_is_schema_error() {
    let content = vec![AssistantContentPart::Text(TextPart::new(
        r#"{"ok": false}"#,
    ))];
    let result = parse_hook_response(&content, coco_types::HookEventType::PreToolUse);
    match result {
        HookEvaluationResult::NonBlockingError { error } => {
            assert!(error.contains("reason"));
        }
        other => panic!("expected NonBlockingError, got {other:?}"),
    }
}

#[test]
fn test_parse_hook_response_invalid_json() {
    let content = vec![AssistantContentPart::Text(TextPart::new("not json"))];
    let result = parse_hook_response(&content, coco_types::HookEventType::PreToolUse);
    match result {
        HookEvaluationResult::NonBlockingError { error } => {
            assert!(
                error.contains("schema validation failed"),
                "unexpected error message: {error}"
            );
        }
        other => panic!("expected NonBlockingError, got {other:?}"),
    }
}

#[test]
fn test_parse_hook_response_empty_text() {
    let content: Vec<AssistantContentPart> = vec![];
    let result = parse_hook_response(&content, coco_types::HookEventType::PreToolUse);
    match result {
        HookEvaluationResult::NonBlockingError { error } => {
            assert!(error.contains("empty assistant text"));
        }
        other => panic!("expected NonBlockingError, got {other:?}"),
    }
}

#[test]
fn test_parse_hook_response_concatenates_multiple_text_parts() {
    let content = vec![
        AssistantContentPart::Text(TextPart::new(r#"{"ok":"#)),
        AssistantContentPart::Text(TextPart::new(r#" true}"#)),
    ];
    let result = parse_hook_response(&content, coco_types::HookEventType::PreToolUse);
    assert!(matches!(result, HookEvaluationResult::Success { .. }));
}

#[test]
fn test_parse_hook_response_ignores_non_text_parts() {
    use coco_llm_types::ReasoningPart;
    let content = vec![
        AssistantContentPart::Reasoning(ReasoningPart::new("thinking…")),
        AssistantContentPart::Text(TextPart::new(r#"{"ok": true}"#)),
    ];
    let result = parse_hook_response(&content, coco_types::HookEventType::PreToolUse);
    assert!(matches!(result, HookEvaluationResult::Success { .. }));
}

#[test]
fn test_build_prompt_shape() {
    let messages = build_prompt(
        "is the file safe?",
        &HookLlmEvaluationContext {
            event: coco_types::HookEventType::PreToolUse,
            hook_input_json: "{}".to_string(),
            transcript_history: vec![],
        },
        STOP_HOOK_TRANSCRIPT_MAX_BYTES,
    );
    assert_eq!(messages.len(), 2);
    matches!(messages[0], LlmMessage::System { .. });
    matches!(messages[1], LlmMessage::User { .. });

    if let LlmMessage::System { content, .. } = &messages[0] {
        let UserContentPart::Text(t) = &content[0] else {
            panic!("expected text part");
        };
        assert!(t.text.contains("evaluating a hook in Claude Code"));
    } else {
        panic!("first message should be System");
    }
}

#[test]
fn stop_build_prompt_uses_transcript_evidence_contract() {
    let messages = build_prompt(
        "tests pass",
        &HookLlmEvaluationContext {
            event: coco_types::HookEventType::Stop,
            hook_input_json: r#"{"ignored":true}"#.to_string(),
            transcript_history: vec!["assistant: cargo test passed".to_string()],
        },
        STOP_HOOK_TRANSCRIPT_MAX_BYTES,
    );

    let LlmMessage::System {
        content: system, ..
    } = &messages[0]
    else {
        panic!("first message should be System");
    };
    let UserContentPart::Text(system_text) = &system[0] else {
        panic!("expected text part");
    };
    assert!(system_text.text.contains("stop-condition hook"));
    assert!(system_text.text.contains("impossible"));

    let LlmMessage::User { content, .. } = &messages[1] else {
        panic!("second message should be User");
    };
    let UserContentPart::Text(user_text) = &content[0] else {
        panic!("expected text part");
    };
    assert!(user_text.text.contains("assistant: cargo test passed"));
    assert!(user_text.text.contains("Condition: tests pass"));
    assert!(!user_text.text.contains("Hook input JSON"));
}

#[test]
fn stop_build_prompt_bounds_transcript_to_recent_evidence() {
    let messages = build_prompt(
        "recent condition",
        &HookLlmEvaluationContext {
            event: coco_types::HookEventType::Stop,
            hook_input_json: "{}".to_string(),
            transcript_history: vec![
                "old evidence should be omitted".repeat(10),
                "middle evidence should be omitted".repeat(10),
                "assistant: recent condition satisfied".to_string(),
            ],
        },
        80,
    );

    let LlmMessage::User { content, .. } = &messages[1] else {
        panic!("second message should be User");
    };
    let UserContentPart::Text(user_text) = &content[0] else {
        panic!("expected text part");
    };
    assert!(user_text.text.contains("older transcript entries omitted"));
    assert!(user_text.text.contains("recent condition satisfied"));
    assert!(!user_text.text.contains("old evidence should be omitted"));
}

#[test]
fn hook_response_format_requests_json_schema() {
    let ResponseFormat::Json {
        schema: Some(schema),
        name: Some(name),
        ..
    } = hook_response_format()
    else {
        panic!("expected named JSON schema response format");
    };
    assert_eq!(name, "hook_verdict");
    assert_eq!(schema["required"], json!(["ok", "reason"]));
    assert_eq!(schema["additionalProperties"], json!(false));
}

#[test]
fn stop_response_requires_reason_and_accepts_impossible() {
    let content = vec![AssistantContentPart::Text(TextPart::new(
        r#"{"ok": false, "reason": "blocked", "impossible": true}"#,
    ))];
    let result = parse_hook_response(&content, coco_types::HookEventType::Stop);
    match result {
        HookEvaluationResult::Impossible { reason } => assert_eq!(reason, "blocked"),
        other => panic!("expected Impossible, got {other:?}"),
    }
}

#[test]
fn non_stop_rejects_impossible() {
    let content = vec![AssistantContentPart::Text(TextPart::new(
        r#"{"ok": false, "reason": "blocked", "impossible": true}"#,
    ))];
    let result = parse_hook_response(&content, coco_types::HookEventType::PreToolUse);
    assert!(matches!(
        result,
        HookEvaluationResult::NonBlockingError { .. }
    ));
}
