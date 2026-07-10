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
            "properties": { "city": { "type": "string" } }
        }),
        input_examples: None,
        strict: Some(true),
        provider_options: None,
    })
}

#[test]
fn no_tools_returns_none() {
    let result = prepare_groq_tools(&None, &None, "llama-3.3-70b-versatile");
    assert!(result.tools.is_none());
    assert!(result.tool_choice.is_none());
    assert!(result.warnings.is_empty());
}

#[test]
fn empty_tools_returns_none() {
    let result = prepare_groq_tools(&Some(vec![]), &None, "llama-3.3-70b-versatile");
    assert!(result.tools.is_none());
}

#[test]
fn converts_function_tool() {
    let result = prepare_groq_tools(
        &Some(vec![function_tool()]),
        &None,
        "llama-3.3-70b-versatile",
    );
    let tools = result.tools.expect("tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "get_weather");
    assert_eq!(tools[0]["function"]["description"], "Get weather");
    assert_eq!(tools[0]["function"]["strict"], true);
}

#[test]
fn browser_search_on_supported_model() {
    let tool =
        LanguageModelV4Tool::Provider(LanguageModelV4ProviderTool::new("groq", "browser_search"));
    let result = prepare_groq_tools(&Some(vec![tool]), &None, "openai/gpt-oss-20b");
    let tools = result.tools.expect("tools");
    assert_eq!(tools, vec![serde_json::json!({"type": "browser_search"})]);
    assert!(result.warnings.is_empty());
}

#[test]
fn browser_search_on_unsupported_model_warns() {
    let tool =
        LanguageModelV4Tool::Provider(LanguageModelV4ProviderTool::new("groq", "browser_search"));
    let result = prepare_groq_tools(&Some(vec![tool]), &None, "llama-3.3-70b-versatile");
    // No browser_search tool emitted (all filtered) → no `tools`, but a warning.
    assert!(result.tools.is_none());
    assert_eq!(result.warnings.len(), 1);
}

#[test]
fn unknown_provider_tool_warns() {
    let tool = LanguageModelV4Tool::Provider(LanguageModelV4ProviderTool::from_id(
        "groq.unknown_tool",
        "unknown_tool",
    ));
    let result = prepare_groq_tools(&Some(vec![tool]), &None, "openai/gpt-oss-20b");
    assert!(result.tools.is_none());
    assert_eq!(result.warnings.len(), 1);
}

#[test]
fn maps_tool_choice_variants() {
    let auto = prepare_groq_tools(&None, &Some(LanguageModelV4ToolChoice::Auto), "m");
    assert_eq!(auto.tool_choice, None); // no tools → choice dropped

    let with_tool = prepare_groq_tools(
        &Some(vec![function_tool()]),
        &Some(LanguageModelV4ToolChoice::Tool {
            tool_name: "get_weather".into(),
        }),
        "llama-3.3-70b-versatile",
    );
    let tc = with_tool.tool_choice.expect("tool_choice");
    assert_eq!(tc["type"], "function");
    assert_eq!(tc["function"]["name"], "get_weather");
}
