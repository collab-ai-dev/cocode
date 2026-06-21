use super::*;
use vercel_ai_provider::language_model::v4::function_tool::LanguageModelV4FunctionTool;

#[test]
fn no_tools_returns_none() {
    let result = prepare_chat_tools(&None, &None);
    assert!(result.tools.is_none());
    assert!(result.tool_choice.is_none());
    assert!(result.warnings.is_empty());
}

#[test]
fn empty_tools_returns_none() {
    let result = prepare_chat_tools(&Some(vec![]), &None);
    assert!(result.tools.is_none());
}

#[test]
fn converts_function_tool() {
    let tool = LanguageModelV4Tool::Function(LanguageModelV4FunctionTool {
        name: "get_weather".into(),
        description: Some("Get weather".into()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "city": { "type": "string" }
            }
        }),
        input_examples: None,
        strict: Some(true),
        provider_options: None,
    });
    let result = prepare_chat_tools(&Some(vec![tool]), &None);
    let tools = result.tools.expect("should have tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "get_weather");
    assert_eq!(tools[0]["function"]["strict"], true);
}

#[test]
fn maps_tool_choice_auto() {
    let result = prepare_chat_tools(&None, &Some(LanguageModelV4ToolChoice::Auto));
    assert_eq!(result.tool_choice, Some(serde_json::json!("auto")));
}

#[test]
fn maps_tool_choice_required() {
    let result = prepare_chat_tools(&None, &Some(LanguageModelV4ToolChoice::Required));
    assert_eq!(result.tool_choice, Some(serde_json::json!("required")));
}

#[test]
fn maps_tool_choice_specific() {
    let result = prepare_chat_tools(
        &None,
        &Some(LanguageModelV4ToolChoice::Tool {
            tool_name: "get_weather".into(),
        }),
    );
    let tc = result.tool_choice.expect("should have tool_choice");
    assert_eq!(tc["type"], "function");
    assert_eq!(tc["function"]["name"], "get_weather");
}
