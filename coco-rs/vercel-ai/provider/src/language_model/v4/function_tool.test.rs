use super::*;
use serde_json::json;
use std::collections::HashMap;

#[test]
fn test_function_tool_new() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "query": { "type": "string" }
        },
        "required": ["query"]
    });
    let tool = LanguageModelV4FunctionTool::new("search", schema);
    assert_eq!(tool.name, "search");
    assert!(tool.description.is_none());
}

#[test]
fn test_function_tool_with_description() {
    let schema = serde_json::json!({ "type": "object" });
    let tool = LanguageModelV4FunctionTool::with_description(
        "get_weather",
        "Get the current weather for a location",
        schema,
    );
    assert_eq!(tool.name, "get_weather");
    assert_eq!(
        tool.description,
        Some("Get the current weather for a location".to_string())
    );
}

#[test]
fn test_function_tool_with_example() {
    let schema = serde_json::json!({ "type": "object" });
    let mut example = HashMap::new();
    example.insert("query".to_string(), json!("test"));
    let tool = LanguageModelV4FunctionTool::new("tool", schema).with_example(example);
    assert!(tool.input_examples.is_some());
    let examples = tool.input_examples.unwrap();
    assert_eq!(examples.len(), 1);
}

#[test]
fn test_function_tool_with_strict() {
    let schema = serde_json::json!({ "type": "object" });
    let tool = LanguageModelV4FunctionTool::new("tool", schema).with_strict(true);
    assert_eq!(tool.strict, Some(true));
}

#[test]
fn test_function_tool_serialization() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "city": { "type": "string" }
        },
        "required": ["city"]
    });
    let tool = LanguageModelV4FunctionTool::with_description("get_weather", "Get weather", schema);
    let json = serde_json::to_string(&tool).unwrap();
    assert!(json.contains(r#""name":"get_weather"#));
    assert!(json.contains(r#""description":"Get weather"#));
    assert!(json.contains(r#""inputSchema"#));

    // When wrapped in LanguageModelV4Tool, the "type" tag is included
    let wrapped = super::super::tool::LanguageModelV4Tool::function(tool);
    let json = serde_json::to_string(&wrapped).unwrap();
    assert!(json.contains(r#""type":"function"#));
}

#[test]
fn test_tool_input_example() {
    let mut input = HashMap::new();
    input.insert("key".to_string(), json!("value"));
    let example = ToolInputExample::new(input);
    assert!(example.input.contains_key("key"));
}
