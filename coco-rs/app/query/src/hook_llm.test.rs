use super::*;
use coco_llm_types::TextPart;

#[test]
fn test_parse_hook_response_ok_true() {
    let content = vec![AssistantContentPart::Text(TextPart::new(r#"{"ok": true}"#))];
    let result = parse_hook_response(&content);
    matches!(result, HookEvaluationResult::Ok);
}

#[test]
fn test_parse_hook_response_ok_false_with_reason() {
    let content = vec![AssistantContentPart::Text(TextPart::new(
        r#"{"ok": false, "reason": "found AWS key"}"#,
    ))];
    let result = parse_hook_response(&content);
    match result {
        HookEvaluationResult::Blocking { reason } => assert_eq!(reason, "found AWS key"),
        other => panic!("expected Blocking, got {other:?}"),
    }
}

#[test]
fn test_parse_hook_response_ok_false_default_reason() {
    let content = vec![AssistantContentPart::Text(TextPart::new(
        r#"{"ok": false}"#,
    ))];
    let result = parse_hook_response(&content);
    match result {
        HookEvaluationResult::Blocking { reason } => {
            assert_eq!(reason, "Prompt hook condition not met")
        }
        other => panic!("expected Blocking with default reason, got {other:?}"),
    }
}

#[test]
fn test_parse_hook_response_invalid_json() {
    let content = vec![AssistantContentPart::Text(TextPart::new("not json"))];
    let result = parse_hook_response(&content);
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
    let result = parse_hook_response(&content);
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
    let result = parse_hook_response(&content);
    matches!(result, HookEvaluationResult::Ok);
}

#[test]
fn test_parse_hook_response_ignores_non_text_parts() {
    use coco_llm_types::ReasoningPart;
    let content = vec![
        AssistantContentPart::Reasoning(ReasoningPart::new("thinking…")),
        AssistantContentPart::Text(TextPart::new(r#"{"ok": true}"#)),
    ];
    let result = parse_hook_response(&content);
    matches!(result, HookEvaluationResult::Ok);
}

#[test]
fn test_build_prompt_shape() {
    let messages = build_prompt("is the file safe?");
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
