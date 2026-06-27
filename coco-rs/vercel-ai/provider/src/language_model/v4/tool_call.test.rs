use super::*;

#[test]
fn test_tool_call_new() {
    let call = LanguageModelV4ToolCall::new("call-1", "get_weather", r#"{"city":"London"}"#);
    assert_eq!(call.tool_call_id, "call-1");
    assert_eq!(call.tool_name, "get_weather");
    assert_eq!(call.input, r#"{"city":"London"}"#);
    assert!(call.provider_executed.is_none());
    assert!(call.dynamic.is_none());
}

#[test]
fn test_tool_call_from_json() {
    let call = LanguageModelV4ToolCall::from_json(
        "call-2",
        "search",
        serde_json::json!({"query": "test"}),
    );
    assert_eq!(call.tool_call_id, "call-2");
    assert_eq!(call.tool_name, "search");
    assert!(call.input.contains("query"));
}

#[test]
fn test_tool_call_with_provider_executed() {
    let call = LanguageModelV4ToolCall::new("call-1", "tool", "{}").with_provider_executed(true);
    assert_eq!(call.provider_executed, Some(true));
}

#[test]
fn test_tool_call_with_dynamic() {
    let call = LanguageModelV4ToolCall::new("call-1", "tool", "{}").with_dynamic(true);
    assert_eq!(call.dynamic, Some(true));
}

#[test]
fn test_tool_call_serialization() {
    let call = LanguageModelV4ToolCall::new("call-1", "get_weather", r#"{"city":"Paris"}"#);
    let json = serde_json::to_string(&call).unwrap();
    assert!(json.contains(r#""toolCallId":"call-1"#));
    assert!(json.contains(r#""toolName":"get_weather"#));
}

#[test]
fn test_tool_call_with_all_options_serialization() {
    let call = LanguageModelV4ToolCall::new("call-1", "tool", "{}")
        .with_provider_executed(true)
        .with_dynamic(false);
    let json = serde_json::to_string(&call).unwrap();
    assert!(json.contains(r#""providerExecuted":true"#));
    assert!(json.contains(r#""dynamic":false"#));
}
