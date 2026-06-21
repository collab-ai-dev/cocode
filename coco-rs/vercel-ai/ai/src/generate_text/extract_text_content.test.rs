use super::*;
use serde_json::json;
use vercel_ai_provider::ReasoningPart;
use vercel_ai_provider::TextPart;

#[test]
fn test_extract_text_content_empty() {
    let content: Vec<AssistantContentPart> = vec![];
    assert_eq!(extract_text_content(&content), "");
}

#[test]
fn test_extract_text_content_single() {
    let content = vec![AssistantContentPart::Text(TextPart {
        text: "Hello, world!".to_string(),
        provider_metadata: None,
    })];
    assert_eq!(extract_text_content(&content), "Hello, world!");
}

#[test]
fn test_extract_text_content_multiple() {
    let content = vec![
        AssistantContentPart::Text(TextPart {
            text: "Hello, ".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::Text(TextPart {
            text: "world!".to_string(),
            provider_metadata: None,
        }),
    ];
    assert_eq!(extract_text_content(&content), "Hello, world!");
}

#[test]
fn test_extract_text_content_mixed() {
    let content = vec![
        AssistantContentPart::Text(TextPart {
            text: "Some text".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "Thinking...".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::Text(TextPart {
            text: " more text".to_string(),
            provider_metadata: None,
        }),
    ];
    assert_eq!(extract_text_content(&content), "Some text more text");
}

#[test]
fn test_extract_text_content_with_metadata() {
    let content = vec![
        AssistantContentPart::Text(TextPart {
            text: "Hello".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "thinking".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::ToolCall(vercel_ai_provider::ToolCallPart {
            tool_call_id: "call_1".to_string(),
            tool_name: "test_tool".to_string(),
            input: json!({}),
            provider_executed: None,
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        }),
    ];

    let (text, has_reasoning, has_tool_calls) = extract_text_content_with_metadata(&content);
    assert_eq!(text, "Hello");
    assert!(has_reasoning);
    assert!(has_tool_calls);
}
