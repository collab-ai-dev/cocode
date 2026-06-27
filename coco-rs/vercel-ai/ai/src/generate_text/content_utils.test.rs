use super::*;
use serde_json::json;
use vercel_ai_provider::ReasoningPart;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::ToolCallPart;

#[test]
fn test_extract_text_empty() {
    let content: Vec<AssistantContentPart> = vec![];
    assert_eq!(extract_text(&content), "");
}

#[test]
fn test_extract_text_single() {
    let content = vec![AssistantContentPart::Text(TextPart {
        text: "Hello".to_string(),
        provider_metadata: None,
    })];
    assert_eq!(extract_text(&content), "Hello");
}

#[test]
fn test_extract_text_multiple() {
    let content = vec![
        AssistantContentPart::text("Hello, "),
        AssistantContentPart::text("world!"),
    ];
    assert_eq!(extract_text(&content), "Hello, world!");
}

#[test]
fn test_extract_text_mixed() {
    let content = vec![
        AssistantContentPart::text("Start"),
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "thinking".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::text(" end"),
    ];
    assert_eq!(extract_text(&content), "Start end");
}

#[test]
fn test_extract_reasoning() {
    let content = vec![
        AssistantContentPart::text("text"),
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "thought 1".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "thought 2".to_string(),
            provider_metadata: None,
        }),
    ];
    assert_eq!(extract_reasoning(&content), vec!["thought 1", "thought 2"]);
}

#[test]
fn test_extract_tool_calls() {
    let content = vec![
        AssistantContentPart::text("text"),
        AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: "call_1".to_string(),
            tool_name: "my_tool".to_string(),
            input: json!({"key": "value"}),
            provider_executed: None,
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        }),
    ];
    let calls = extract_tool_calls(&content);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].tool_call_id, "call_1");
    assert_eq!(calls[0].tool_name, "my_tool");
}

#[test]
fn test_extract_tool_calls_empty() {
    let content = vec![AssistantContentPart::text("no tools")];
    assert!(extract_tool_calls(&content).is_empty());
}

#[test]
fn test_extract_reasoning_outputs() {
    let content = vec![
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "thought".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::text("text"),
    ];
    let outputs = extract_reasoning_outputs(&content);
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].text, "thought");
    assert!(outputs[0].provider_metadata.is_none());
}
