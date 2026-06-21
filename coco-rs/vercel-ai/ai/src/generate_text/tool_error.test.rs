use super::*;

#[test]
fn test_tool_error_new() {
    let error = ToolError::new(
        "call_123",
        "my_tool",
        serde_json::json!("Something went wrong"),
    );
    assert_eq!(error.tool_call_id, "call_123");
    assert_eq!(error.tool_name, "my_tool");
    assert_eq!(error.message(), "Something went wrong");
    assert!(!error.dynamic);
}

#[test]
fn test_tool_error_from_message() {
    let error = ToolError::from_message("Generic error");
    assert!(error.tool_call_id.is_empty());
    assert!(error.tool_name.is_empty());
    assert_eq!(error.message(), "Generic error");
}

#[test]
fn test_tool_error_builder() {
    let error = ToolError::from_message("Error")
        .with_tool_call_id("call_1")
        .with_tool_name("test_tool")
        .with_input(serde_json::json!({"key": "value"}))
        .with_provider_executed(true)
        .as_dynamic()
        .with_title("Custom Title");

    assert_eq!(error.tool_call_id, "call_1");
    assert_eq!(error.tool_name, "test_tool");
    assert!(error.dynamic);
    assert_eq!(error.title, Some("Custom Title".to_string()));
    assert_eq!(error.provider_executed, Some(true));
    assert_eq!(error.input, serde_json::json!({"key": "value"}));
}

#[test]
fn test_tool_error_display() {
    let error1 = ToolError::from_message("Error");
    assert_eq!(error1.to_string(), "Tool error: Error");

    let error2 = ToolError::new("", "my_tool", serde_json::json!("Failed"));
    assert_eq!(error2.to_string(), "Tool 'my_tool' error: Failed");

    let error3 = ToolError::new("call_1", "my_tool", serde_json::json!("Failed"));
    assert_eq!(error3.to_string(), "Tool 'my_tool' (call_1) error: Failed");
}

#[test]
fn test_tool_error_functions() {
    let err1 = tool_error("Simple error");
    assert_eq!(err1.message(), "Simple error");

    let err2 = tool_error_with_context("id_1", "tool_1", "Context error");
    assert_eq!(err2.tool_call_id, "id_1");
    assert_eq!(err2.tool_name, "tool_1");
}
