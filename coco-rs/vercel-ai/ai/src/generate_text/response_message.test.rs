use super::*;
use serde_json::json;

#[test]
fn test_build_assistant_message() {
    let content = vec![AssistantContentPart::text("Hello")];
    let message = build_assistant_message(content);

    match message {
        LanguageModelV4Message::Assistant { content: c, .. } => {
            assert_eq!(c.len(), 1);
        }
        _ => panic!("Expected assistant message"),
    }
}

#[test]
fn test_build_assistant_message_from_text() {
    let message = build_assistant_message_from_text("Hello, world!");

    match message {
        LanguageModelV4Message::Assistant { content, .. } => {
            assert_eq!(content.len(), 1);
        }
        _ => panic!("Expected assistant message"),
    }
}

#[test]
fn test_build_tool_result_message() {
    let tool_results = vec![
        ToolResult::new("call_1", "tool_a", json!({ "result": 42 })),
        ToolResult::new("call_2", "tool_b", json!({ "status": "ok" })),
    ];

    let message = build_tool_result_message(&tool_results);

    match message {
        LanguageModelV4Message::Tool { content, .. } => {
            assert_eq!(content.len(), 2);
        }
        _ => panic!("Expected tool message"),
    }
}

#[test]
fn test_build_single_tool_result_message() {
    let message =
        build_single_tool_result_message("call_123", "my_tool", json!({ "answer": 42 }), false);

    match message {
        LanguageModelV4Message::Tool { content, .. } => {
            assert_eq!(content.len(), 1);
        }
        _ => panic!("Expected tool message"),
    }
}

#[test]
fn test_build_single_tool_result_message_error() {
    let message =
        build_single_tool_result_message("call_123", "my_tool", json!({ "error": "failed" }), true);

    match message {
        LanguageModelV4Message::Tool { content, .. } => {
            assert_eq!(content.len(), 1);
        }
        _ => panic!("Expected tool message"),
    }
}

#[test]
fn test_response_message_data() {
    let data = ResponseMessageData::new(
        vec![AssistantContentPart::text("Hello")],
        FinishReason::end_turn(),
        Usage::default(),
    );

    assert!(!data.has_tool_calls());
    assert!(!data.has_tool_results());
    assert_eq!(data.to_messages().len(), 1);
}

#[test]
fn test_response_message_data_with_tools() {
    let tool_result = ToolResult::new("call_1", "tool_a", json!({}));

    let data = ResponseMessageData::new(
        vec![AssistantContentPart::text("Result:")],
        FinishReason::tool_use(),
        Usage::default(),
    )
    .with_tool_results(vec![tool_result]);

    assert!(data.has_tool_results());
    assert_eq!(data.to_messages().len(), 2); // assistant + tool result
}
