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

fn provider_tool(id: &str, name: &str) -> LanguageModelV4Tool {
    LanguageModelV4Tool::Provider(LanguageModelV4ProviderTool::from_id(id, name))
}

#[test]
fn no_tools_returns_empty() {
    let prepared = prepare_responses_tools(&None, &None);
    assert!(prepared.tools.is_empty());
    assert!(prepared.tool_choice.is_none());
    assert!(prepared.warnings.is_empty());
}

#[test]
fn function_tool_is_sanitized() {
    let prepared = prepare_responses_tools(&Some(vec![function_tool()]), &None);
    assert_eq!(prepared.tools.len(), 1);
    assert_eq!(prepared.tools[0]["type"], "function");
    assert_eq!(prepared.tools[0]["name"], "get_weather");
    assert_eq!(prepared.tools[0]["strict"], true);
    assert!(
        prepared.tools[0]["parameters"]
            .get("additionalProperties")
            .is_none()
    );
}

#[test]
fn provider_tools_map_by_id() {
    let tools = vec![
        provider_tool("xai.web_search", "web_search"),
        provider_tool("xai.x_search", "x_search"),
        provider_tool("xai.code_execution", "code"),
        provider_tool("xai.view_image", "vi"),
        provider_tool("xai.view_x_video", "vxv"),
        provider_tool("xai.mcp", "mcp"),
    ];
    let prepared = prepare_responses_tools(&Some(tools), &None);
    let types: Vec<&str> = prepared
        .tools
        .iter()
        .map(|t| t["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        vec![
            "web_search",
            "x_search",
            "code_interpreter",
            "view_image",
            "view_x_video",
            "mcp",
        ]
    );
    assert!(prepared.warnings.is_empty());
}

#[test]
fn file_search_forwards_args() {
    let mut pt = LanguageModelV4ProviderTool::from_id("xai.file_search", "file_search");
    pt.args
        .insert("vectorStoreIds".into(), serde_json::json!(["vs_1"]));
    pt.args.insert("maxNumResults".into(), serde_json::json!(4));
    let prepared = prepare_responses_tools(&Some(vec![LanguageModelV4Tool::Provider(pt)]), &None);
    assert_eq!(prepared.tools[0]["type"], "file_search");
    assert_eq!(prepared.tools[0]["vector_store_ids"][0], "vs_1");
    assert_eq!(prepared.tools[0]["max_num_results"], 4);
}

#[test]
fn unknown_provider_tool_warns() {
    let prepared =
        prepare_responses_tools(&Some(vec![provider_tool("xai.unknown", "mystery")]), &None);
    assert!(prepared.tools.is_empty());
    assert_eq!(prepared.warnings.len(), 1);
}

#[test]
fn tool_choice_required_and_named() {
    let required = prepare_responses_tools(
        &Some(vec![function_tool()]),
        &Some(LanguageModelV4ToolChoice::Required),
    );
    assert_eq!(required.tool_choice, Some(serde_json::json!("required")));

    let named = prepare_responses_tools(
        &Some(vec![function_tool()]),
        &Some(LanguageModelV4ToolChoice::Tool {
            tool_name: "get_weather".into(),
        }),
    );
    let tc = named.tool_choice.expect("tool_choice");
    assert_eq!(
        tc,
        serde_json::json!({ "type": "function", "name": "get_weather" })
    );
}

#[test]
fn tool_choice_for_server_tool_warns_and_drops() {
    let tools = vec![provider_tool("xai.web_search", "web_search")];
    let prepared = prepare_responses_tools(
        &Some(tools),
        &Some(LanguageModelV4ToolChoice::Tool {
            tool_name: "web_search".into(),
        }),
    );
    assert!(prepared.tool_choice.is_none());
    assert_eq!(prepared.warnings.len(), 1);
}

#[test]
fn tool_choice_unknown_name_dropped_silently() {
    let prepared = prepare_responses_tools(
        &Some(vec![function_tool()]),
        &Some(LanguageModelV4ToolChoice::Tool {
            tool_name: "nope".into(),
        }),
    );
    assert!(prepared.tool_choice.is_none());
    assert!(prepared.warnings.is_empty());
}
