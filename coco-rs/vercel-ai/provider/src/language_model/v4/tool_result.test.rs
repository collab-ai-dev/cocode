use super::*;

#[test]
fn test_tool_result_new() {
    let result =
        LanguageModelV4ToolResult::new("call-1", "get_weather", serde_json::json!({"temp": 20}));
    assert_eq!(result.tool_call_id, "call-1");
    assert_eq!(result.tool_name, "get_weather");
    assert!(result.is_error.is_none());
    assert!(result.preliminary.is_none());
}

#[test]
fn test_tool_result_error() {
    let result =
        LanguageModelV4ToolResult::error("call-1", "tool", serde_json::json!({"error": "failed"}));
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn test_tool_result_with_preliminary() {
    let result = LanguageModelV4ToolResult::new("call-1", "tool", serde_json::json!({}))
        .with_preliminary(true);
    assert_eq!(result.preliminary, Some(true));
}

#[test]
fn test_tool_result_with_dynamic() {
    let result =
        LanguageModelV4ToolResult::new("call-1", "tool", serde_json::json!({})).with_dynamic(true);
    assert_eq!(result.dynamic, Some(true));
}

#[test]
fn test_tool_result_serialization() {
    let result =
        LanguageModelV4ToolResult::new("call-1", "get_weather", serde_json::json!({"temp": 25}));
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains(r#""toolCallId":"call-1"#));
    assert!(json.contains(r#""toolName":"get_weather"#));
}

#[test]
fn test_tool_result_with_all_options_serialization() {
    let result = LanguageModelV4ToolResult::new("call-1", "tool", serde_json::json!({}))
        .with_error(true)
        .with_preliminary(false)
        .with_dynamic(true);
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains(r#""isError":true"#));
    assert!(json.contains(r#""preliminary":false"#));
    assert!(json.contains(r#""dynamic":true"#));
}
