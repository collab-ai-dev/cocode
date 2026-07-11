use super::*;
use pretty_assertions::assert_eq;
use vercel_ai_provider::LanguageModelV4ProviderTool;
use vercel_ai_provider::language_model::v4::function_tool::LanguageModelV4FunctionTool;

fn function_tool() -> LanguageModelV4Tool {
    LanguageModelV4Tool::Function(LanguageModelV4FunctionTool {
        name: "get_weather".into(),
        description: Some("Get weather".into()),
        input_schema: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": { "city": { "type": "string" } }
        }),
        input_examples: None,
        strict: Some(true),
        provider_options: None,
    })
}

#[test]
fn no_tools_returns_none() {
    let result = prepare_xai_tools(&None, &None);
    assert!(result.tools.is_none());
    assert!(result.tool_choice.is_none());
    assert!(result.warnings.is_empty());
}

#[test]
fn empty_tools_returns_none() {
    let result = prepare_xai_tools(&Some(vec![]), &None);
    assert!(result.tools.is_none());
}

#[test]
fn converts_function_tool_and_strips_additional_properties() {
    let result = prepare_xai_tools(&Some(vec![function_tool()]), &None);
    let tools = result.tools.expect("tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "get_weather");
    assert_eq!(tools[0]["function"]["description"], "Get weather");
    assert_eq!(tools[0]["function"]["strict"], true);
    // additionalProperties: false must be sanitized away.
    assert!(
        tools[0]["function"]["parameters"]
            .get("additionalProperties")
            .is_none()
    );
    assert_eq!(
        tools[0]["function"]["parameters"]["properties"]["city"]["type"],
        "string"
    );
}

#[test]
fn provider_tool_warns_and_is_dropped() {
    let tool = LanguageModelV4Tool::Provider(LanguageModelV4ProviderTool::new("xai", "web_search"));
    let result = prepare_xai_tools(&Some(vec![tool]), &None);
    assert!(result.tools.is_none());
    assert_eq!(result.warnings.len(), 1);
}

#[test]
fn maps_tool_choice_variants() {
    let required = prepare_xai_tools(
        &Some(vec![function_tool()]),
        &Some(LanguageModelV4ToolChoice::Required),
    );
    assert_eq!(required.tool_choice, Some(serde_json::json!("required")));

    let with_tool = prepare_xai_tools(
        &Some(vec![function_tool()]),
        &Some(LanguageModelV4ToolChoice::Tool {
            tool_name: "get_weather".into(),
        }),
    );
    let tc = with_tool.tool_choice.expect("tool_choice");
    assert_eq!(tc["type"], "function");
    assert_eq!(tc["function"]["name"], "get_weather");
}

#[test]
fn tool_choice_dropped_when_no_tools() {
    let auto = prepare_xai_tools(&None, &Some(LanguageModelV4ToolChoice::Auto));
    assert_eq!(auto.tool_choice, None);
}
