use super::*;
use vercel_ai_provider::LanguageModelV4ProviderTool;
use vercel_ai_provider::language_model::v4::function_tool::LanguageModelV4FunctionTool;

#[test]
fn no_tools() {
    let r = prepare_responses_tools(&None, &None);
    assert!(r.tools.is_none());
    assert!(r.tool_choice.is_none());
}

#[test]
fn function_tool() {
    let tool = LanguageModelV4Tool::Function(LanguageModelV4FunctionTool {
        name: "get_weather".into(),
        description: Some("Get weather".into()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "city": { "type": "string" } }
        }),
        input_examples: None,
        strict: Some(true),
        provider_options: None,
    });
    let r = prepare_responses_tools(&Some(vec![tool]), &None);
    let tools = r.tools.expect("should have tools");
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["name"], "get_weather");
    assert_eq!(tools[0]["strict"], true);
}

#[test]
fn provider_tool_web_search() {
    let tool = LanguageModelV4Tool::Provider(LanguageModelV4ProviderTool {
        id: "openai.web_search".into(),
        name: "web_search".into(),
        args: [("search_context_size".into(), serde_json::json!("medium"))]
            .into_iter()
            .collect(),
    });
    let r = prepare_responses_tools(&Some(vec![tool]), &None);
    let tools = r.tools.expect("should have tools");
    assert_eq!(tools[0]["type"], "web_search");
    assert_eq!(tools[0]["search_context_size"], "medium");
}

#[test]
fn provider_tool_apply_patch_custom_grammar() {
    // coco's freeform apply_patch: a provider-defined custom tool carrying the
    // lark grammar. `id: "openai.custom"` is mandatory — `id:"openai.apply_patch"`
    // would hit the built-in `{type:"apply_patch"}` path and drop the name/format.
    let tool = LanguageModelV4Tool::Provider(LanguageModelV4ProviderTool {
        id: "openai.custom".into(),
        name: "apply_patch".into(),
        args: [
            ("description".into(), serde_json::json!("Use apply_patch")),
            (
                "format".into(),
                serde_json::json!({
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": "start: \"*** Begin Patch\"",
                }),
            ),
        ]
        .into_iter()
        .collect(),
    });
    let r = prepare_responses_tools(&Some(vec![tool]), &None);
    let tools = r.tools.expect("should have tools");
    assert_eq!(tools[0]["type"], "custom");
    assert_eq!(tools[0]["name"], "apply_patch");
    assert_eq!(tools[0]["format"]["type"], "grammar");
    assert_eq!(tools[0]["format"]["syntax"], "lark");
}

#[test]
fn provider_tool_args_serialize_in_sorted_key_order() {
    // Prompt-cache stability: `pt.args` is a HashMap whose iteration order
    // varies per instance, and serde_json preserves insertion order — so the
    // wire object's arg keys MUST be sorted deterministically, not left in
    // HashMap order (which would change the request-prefix bytes turn-to-turn).
    let tool = LanguageModelV4Tool::Provider(LanguageModelV4ProviderTool {
        id: "openai.custom".into(),
        name: "apply_patch".into(),
        args: [
            ("format".into(), serde_json::json!({ "type": "grammar" })),
            ("description".into(), serde_json::json!("d")),
        ]
        .into_iter()
        .collect(),
    });
    let r = prepare_responses_tools(&Some(vec![tool]), &None);
    let obj = r.tools.unwrap()[0].as_object().unwrap().clone();
    // Among the arg keys, `description` must precede `format` (sorted).
    let arg_keys: Vec<&str> = obj
        .keys()
        .map(String::as_str)
        .filter(|k| matches!(*k, "description" | "format"))
        .collect();
    assert_eq!(
        arg_keys,
        vec!["description", "format"],
        "provider-tool args must serialize in sorted key order for cache stability"
    );
}
