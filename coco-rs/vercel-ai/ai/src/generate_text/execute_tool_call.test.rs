use super::*;
use serde_json::json;

fn make_tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall::new(id, name, json!({}))
}

#[test]
fn test_validate_tool_call() {
    let tools = Arc::new(ToolRegistry::new());
    let tc = make_tool_call("id_1", "nonexistent_tool");

    // Tool doesn't exist
    let result = validate_tool_call(&tc, &tools);
    assert!(result.is_err());
}

#[test]
fn test_output_to_result_content() {
    let output = ToolOutput::text("result");
    let content = output_to_result_content(&output);

    // ToolResultContent should contain our text
    let text = serde_json::to_string(&content).unwrap();
    assert!(text.contains("result"));
}