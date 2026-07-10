use super::*;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4ProviderTool;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::ResponseFormat;

fn make_config() -> Arc<GroqConfig> {
    Arc::new(GroqConfig {
        provider: "groq.chat".into(),
        base_url: "https://api.groq.com/openai/v1".into(),
        headers: Arc::new(|| {
            let mut h = HashMap::new();
            h.insert("Authorization".into(), "Bearer test".into());
            h
        }),
        client: None,
    })
}

fn model(id: &str) -> GroqChatLanguageModel {
    GroqChatLanguageModel::new(id, make_config())
}

fn groq_provider_options(inner: serde_json::Value) -> ProviderOptions {
    let ns: HashMap<String, serde_json::Value> = serde_json::from_value(inner).unwrap();
    let mut map = HashMap::new();
    map.insert("groq".to_string(), ns);
    ProviderOptions(map)
}

#[test]
fn get_args_basic() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("Hello")],
        temperature: Some(0.5),
        max_output_tokens: Some(100),
        ..Default::default()
    };
    let (body, warnings) = model("llama-3.3-70b-versatile").get_args(&options).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(body["model"], "llama-3.3-70b-versatile");
    assert_eq!(body["temperature"], 0.5);
    assert_eq!(body["max_tokens"], 100);
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"], "Hello");
    assert!(body.get("stream").is_none());
}

#[test]
fn get_args_top_k_warns() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        top_k: Some(5),
        ..Default::default()
    };
    let (_, warnings) = model("llama-3.3-70b-versatile").get_args(&options).unwrap();
    assert_eq!(warnings.len(), 1);
}

#[test]
fn get_args_provider_options_wire() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        provider_options: Some(groq_provider_options(serde_json::json!({
            "reasoningFormat": "parsed",
            "reasoningEffort": "high",
            "serviceTier": "flex",
            "user": "u-9",
            "parallelToolCalls": false
        }))),
        ..Default::default()
    };
    let (body, _) = model("openai/gpt-oss-120b").get_args(&options).unwrap();
    assert_eq!(body["reasoning_format"], "parsed");
    assert_eq!(body["reasoning_effort"], "high");
    assert_eq!(body["service_tier"], "flex");
    assert_eq!(body["user"], "u-9");
    assert_eq!(body["parallel_tool_calls"], false);
}

#[test]
fn reasoning_level_maps_to_effort() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        reasoning: Some(ReasoningLevel::Xhigh),
        ..Default::default()
    };
    let (body, _) = model("openai/gpt-oss-120b").get_args(&options).unwrap();
    // xhigh maps to "high".
    assert_eq!(body["reasoning_effort"], "high");
}

#[test]
fn reasoning_xhigh_maps_to_high_with_compat_warning() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        reasoning: Some(ReasoningLevel::Xhigh),
        ..Default::default()
    };
    let (body, warnings) = model("openai/gpt-oss-120b").get_args(&options).unwrap();
    assert_eq!(body["reasoning_effort"], "high");
    // xhigh has no Groq tier, so mapping to "high" emits a compatibility warning.
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, vercel_ai_provider::Warning::Compatibility { .. }))
    );
}

#[test]
fn reasoning_off_yields_no_effort() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        reasoning: Some(ReasoningLevel::Off),
        ..Default::default()
    };
    let (body, _) = model("openai/gpt-oss-120b").get_args(&options).unwrap();
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn response_format_json_object_without_schema() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        response_format: Some(ResponseFormat::json()),
        ..Default::default()
    };
    let (body, _) = model("llama-3.3-70b-versatile").get_args(&options).unwrap();
    assert_eq!(body["response_format"]["type"], "json_object");
}

#[test]
fn response_format_json_schema_when_structured() {
    let schema = serde_json::json!({"type": "object", "properties": {}});
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        response_format: Some(ResponseFormat::json_with_schema(schema).with_name("out")),
        ..Default::default()
    };
    let (body, _) = model("llama-3.3-70b-versatile").get_args(&options).unwrap();
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["response_format"]["json_schema"]["name"], "out");
    assert_eq!(body["response_format"]["json_schema"]["strict"], true);
}

#[test]
fn response_format_schema_warns_without_structured_outputs() {
    let schema = serde_json::json!({"type": "object"});
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        response_format: Some(ResponseFormat::json_with_schema(schema)),
        provider_options: Some(groq_provider_options(serde_json::json!({
            "structuredOutputs": false
        }))),
        ..Default::default()
    };
    let (body, warnings) = model("llama-3.3-70b-versatile").get_args(&options).unwrap();
    assert_eq!(body["response_format"]["type"], "json_object");
    assert_eq!(warnings.len(), 1);
}

#[test]
fn browser_search_tool_emitted_on_supported_model() {
    let tool =
        LanguageModelV4Tool::Provider(LanguageModelV4ProviderTool::new("groq", "browser_search"));
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("search")],
        tools: Some(vec![tool]),
        ..Default::default()
    };
    let (body, warnings) = model("openai/gpt-oss-20b").get_args(&options).unwrap();
    assert_eq!(body["tools"][0]["type"], "browser_search");
    assert!(warnings.is_empty());
}

#[test]
fn supported_urls_accepts_image_urls() {
    let m = model("llama-3.3-70b-versatile");
    let urls = m.supported_urls();
    assert!(urls.contains_key("image/*"));
    let re = &urls["image/*"][0];
    assert!(re.is_match("https://example.com/a.png"));
    assert!(!re.is_match("ftp://example.com/a.png"));
}
