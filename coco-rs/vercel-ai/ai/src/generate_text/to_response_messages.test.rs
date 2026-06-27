use super::*;
use serde_json::json;

fn make_tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall::new(id, name, json!({ "arg": "value" }))
}

fn make_tool_result(id: &str, name: &str, result: serde_json::Value) -> ToolResult {
    ToolResult::new(id, name, result)
}

#[test]
fn test_to_response_messages_no_tools() {
    let content = vec![AssistantContentPart::text("Hello")];
    let messages = to_response_messages(content, &[]);

    assert!(messages.tool_message.is_none());
    assert_eq!(messages.to_vec().len(), 1);
}

#[test]
fn test_to_response_messages_with_tools() {
    let content = vec![AssistantContentPart::text("Result:")];
    let tool_results = vec![make_tool_result("id_1", "tool_a", json!({ "ok": true }))];

    let messages = to_response_messages(content, &tool_results);

    assert!(messages.tool_message.is_some());
    assert_eq!(messages.to_vec().len(), 2);
}

#[test]
fn test_to_response_messages_from_tool_calls() {
    let tool_calls = vec![make_tool_call("id_1", "tool_a")];
    let tool_results = vec![make_tool_result("id_1", "tool_a", json!({}))];

    let messages = to_response_messages_from_tool_calls(&tool_calls, &tool_results);

    assert!(messages.tool_message.is_some());
}

#[test]
fn test_build_tool_result_message() {
    let tool_results = vec![
        make_tool_result("id_1", "tool_a", json!({ "result": 42 })),
        make_tool_result("id_2", "tool_b", json!({ "status": "ok" })),
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
fn test_build_tool_result_message_with_error() {
    let tool_results = vec![ToolResult::error("id_1", "tool_a", "Something went wrong")];

    let message = build_tool_result_message(&tool_results);

    match message {
        LanguageModelV4Message::Tool { content, .. } => {
            assert_eq!(content.len(), 1);
        }
        _ => panic!("Expected tool message"),
    }
}

#[test]
fn test_to_response_messages_with_text() {
    let tool_calls = vec![make_tool_call("id_1", "tool_a")];
    let tool_results = vec![make_tool_result("id_1", "tool_a", json!({}))];

    let messages =
        to_response_messages_with_text("Here's the result:", &tool_calls, &tool_results);

    assert!(messages.tool_message.is_some());
}

#[test]
fn test_build_text_response() {
    let message = build_text_response("Hello, world!");

    match message {
        LanguageModelV4Message::Assistant { content, .. } => {
            assert_eq!(content.len(), 1);
        }
        _ => panic!("Expected assistant message"),
    }
}