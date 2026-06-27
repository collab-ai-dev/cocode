use super::*;

#[test]
fn test_generate_text_result_new() {
    let result = GenerateTextResult::new(
        "Hello".to_string(),
        Usage::new(10, 5),
        FinishReason::end_turn(),
    );
    assert_eq!(result.text, "Hello");
    assert_eq!(result.usage.total_input_tokens(), 10);
}

#[test]
fn test_tool_call() {
    let tc = ToolCall::new("call_123", "echo", serde_json::json!({"message": "hi"}));
    assert_eq!(tc.tool_call_id, "call_123");
    assert_eq!(tc.tool_name, "echo");
}

#[test]
fn test_tool_result() {
    let tr = ToolResult::new("call_123", "echo", serde_json::json!({"result": "hi"}));
    assert!(!tr.is_error);

    let err = ToolResult::error("call_456", "fail", "Something went wrong");
    assert!(err.is_error);
}
