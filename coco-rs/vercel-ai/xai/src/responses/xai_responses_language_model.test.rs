use super::*;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4ProviderTool;
use vercel_ai_provider::LanguageModelV4ToolChoice;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::ResponseFormat;
use vercel_ai_provider::language_model::v4::function_tool::LanguageModelV4FunctionTool;

fn model() -> XaiResponsesLanguageModel {
    let headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync> = Arc::new(HashMap::new);
    let config = Arc::new(XaiConfig {
        provider: "xai.responses".into(),
        base_url: "https://api.x.ai/v1".into(),
        headers,
        client: None,
    });
    XaiResponsesLanguageModel::new("grok-4.5", config)
}

fn options() -> LanguageModelV4CallOptions {
    LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hello")],
        ..Default::default()
    }
}

fn provider_options(inner: serde_json::Value) -> ProviderOptions {
    let ns: HashMap<String, serde_json::Value> = serde_json::from_value(inner).unwrap();
    let mut map = HashMap::new();
    map.insert("xai".to_string(), ns);
    ProviderOptions(map)
}

#[test]
fn basic_body_has_model_and_input() {
    let (body, warnings, _) = model().get_args(&options()).unwrap();
    assert_eq!(body["model"], "grok-4.5");
    assert_eq!(body["input"][0]["role"], "user");
    assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    assert!(warnings.is_empty());
    assert!(body.get("stream").is_none());
}

#[test]
fn reasoning_effort_and_summary() {
    let mut o = options();
    o.provider_options = Some(provider_options(serde_json::json!({
        "reasoningEffort": "high",
        "reasoningSummary": "detailed",
    })));
    let (body, _, _) = model().get_args(&o).unwrap();
    assert_eq!(body["reasoning"]["effort"], "high");
    assert_eq!(body["reasoning"]["summary"], "detailed");
}

#[test]
fn store_false_forces_encrypted_include() {
    let mut o = options();
    o.provider_options = Some(provider_options(serde_json::json!({ "store": false })));
    let (body, _, _) = model().get_args(&o).unwrap();
    assert_eq!(body["store"], false);
    assert_eq!(body["include"][0], "reasoning.encrypted_content");
}

#[test]
fn store_true_omits_store_and_include() {
    let mut o = options();
    o.provider_options = Some(provider_options(serde_json::json!({ "store": true })));
    let (body, _, _) = model().get_args(&o).unwrap();
    assert!(body.get("store").is_none());
    assert!(body.get("include").is_none());
}

#[test]
fn logprobs_and_top_logprobs() {
    let mut o = options();
    o.provider_options = Some(provider_options(serde_json::json!({ "topLogprobs": 3 })));
    let (body, _, _) = model().get_args(&o).unwrap();
    assert_eq!(body["logprobs"], true);
    assert_eq!(body["top_logprobs"], 3);
}

#[test]
fn previous_response_id_passthrough() {
    let mut o = options();
    o.provider_options = Some(provider_options(
        serde_json::json!({ "previousResponseId": "resp_9" }),
    ));
    let (body, _, _) = model().get_args(&o).unwrap();
    assert_eq!(body["previous_response_id"], "resp_9");
}

#[test]
fn json_schema_response_format_is_strict() {
    let mut o = options();
    o.response_format = Some(ResponseFormat::Json {
        schema: Some(serde_json::json!({ "type": "object" })),
        name: Some("out".into()),
        description: None,
    });
    let (body, _, _) = model().get_args(&o).unwrap();
    assert_eq!(body["text"]["format"]["type"], "json_schema");
    assert_eq!(body["text"]["format"]["strict"], true);
    assert_eq!(body["text"]["format"]["name"], "out");
}

#[test]
fn function_tool_and_tool_choice() {
    let mut o = options();
    o.tools = Some(vec![LanguageModelV4Tool::Function(
        LanguageModelV4FunctionTool {
            name: "get_weather".into(),
            description: None,
            input_schema: serde_json::json!({ "type": "object" }),
            input_examples: None,
            strict: None,
            provider_options: None,
        },
    )]);
    o.tool_choice = Some(LanguageModelV4ToolChoice::Required);
    let (body, _, _) = model().get_args(&o).unwrap();
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["name"], "get_weather");
    assert_eq!(body["tool_choice"], "required");
}

#[test]
fn provider_tool_names_are_collected() {
    let mut o = options();
    let mut pt = LanguageModelV4ProviderTool::from_id("xai.web_search", "myWeb");
    pt.args
        .insert("allowedDomains".into(), serde_json::json!(["a.com"]));
    o.tools = Some(vec![LanguageModelV4Tool::Provider(pt)]);
    let (body, _, names) = model().get_args(&o).unwrap();
    assert_eq!(body["tools"][0]["type"], "web_search");
    assert_eq!(body["tools"][0]["allowed_domains"][0], "a.com");
    assert_eq!(names.web_search.as_deref(), Some("myWeb"));
}

#[test]
fn stop_sequences_warns() {
    let mut o = options();
    o.stop_sequences = Some(vec!["STOP".into()]);
    let (_, warnings, _) = model().get_args(&o).unwrap();
    assert_eq!(warnings.len(), 1);
}

#[test]
fn max_output_tokens_temperature_top_p() {
    let mut o = options();
    o.max_output_tokens = Some(256);
    o.temperature = Some(0.5);
    o.top_p = Some(0.25);
    let (body, _, _) = model().get_args(&o).unwrap();
    assert_eq!(body["max_output_tokens"], 256);
    assert_eq!(body["temperature"], 0.5);
    assert_eq!(body["top_p"], 0.25);
}
