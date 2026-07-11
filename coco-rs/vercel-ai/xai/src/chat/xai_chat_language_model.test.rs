use super::*;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::ResponseFormat;
use vercel_ai_provider::Warning;

fn make_config() -> Arc<XaiConfig> {
    Arc::new(XaiConfig {
        provider: "xai.chat".into(),
        base_url: "https://api.x.ai/v1".into(),
        headers: Arc::new(|| {
            let mut h = HashMap::new();
            h.insert("Authorization".into(), "Bearer test".into());
            h
        }),
        client: None,
    })
}

fn model(id: &str) -> XaiChatLanguageModel {
    XaiChatLanguageModel::new(id, make_config())
}

fn xai_provider_options(inner: serde_json::Value) -> ProviderOptions {
    let ns: HashMap<String, serde_json::Value> = serde_json::from_value(inner).unwrap();
    let mut map = HashMap::new();
    map.insert("xai".to_string(), ns);
    ProviderOptions(map)
}

#[test]
fn get_args_basic_uses_max_completion_tokens() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("Hello")],
        temperature: Some(0.5),
        max_output_tokens: Some(100),
        ..Default::default()
    };
    let (body, warnings) = model("grok-4.5").get_args(&options).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(body["model"], "grok-4.5");
    assert_eq!(body["temperature"], 0.5);
    assert_eq!(body["max_completion_tokens"], 100);
    assert!(body.get("max_tokens").is_none());
    assert_eq!(body["messages"][0]["role"], "user");
    assert!(body.get("stream").is_none());
}

#[test]
fn get_args_warns_on_unsupported_params() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        top_k: Some(5),
        frequency_penalty: Some(0.1),
        presence_penalty: Some(0.2),
        stop_sequences: Some(vec!["END".into()]),
        ..Default::default()
    };
    let (body, warnings) = model("grok-4.5").get_args(&options).unwrap();
    assert_eq!(warnings.len(), 4);
    // None of these unsupported params leak into the body.
    assert!(body.get("top_k").is_none());
    assert!(body.get("frequency_penalty").is_none());
    assert!(body.get("presence_penalty").is_none());
    assert!(body.get("stop").is_none());
}

#[test]
fn get_args_provider_options_wire() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        provider_options: Some(xai_provider_options(serde_json::json!({
            "reasoningEffort": "high",
            "topLogprobs": 5,
            "parallel_function_calling": false
        }))),
        ..Default::default()
    };
    let (body, _) = model("grok-4.5").get_args(&options).unwrap();
    assert_eq!(body["reasoning_effort"], "high");
    // topLogprobs implies logprobs = true.
    assert_eq!(body["logprobs"], true);
    assert_eq!(body["top_logprobs"], 5);
    assert_eq!(body["parallel_function_calling"], false);
}

#[test]
fn logprobs_true_without_top_logprobs() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        provider_options: Some(xai_provider_options(
            serde_json::json!({ "logprobs": true }),
        )),
        ..Default::default()
    };
    let (body, _) = model("grok-4.5").get_args(&options).unwrap();
    assert_eq!(body["logprobs"], true);
    assert!(body.get("top_logprobs").is_none());
}

#[test]
fn reasoning_xhigh_maps_to_high_with_compat_warning() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        reasoning: Some(ReasoningLevel::Xhigh),
        ..Default::default()
    };
    let (body, warnings) = model("grok-4.5").get_args(&options).unwrap();
    assert_eq!(body["reasoning_effort"], "high");
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, Warning::Compatibility { .. }))
    );
}

#[test]
fn reasoning_off_maps_to_none() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        reasoning: Some(ReasoningLevel::Off),
        ..Default::default()
    };
    let (body, _) = model("grok-4.5").get_args(&options).unwrap();
    assert_eq!(body["reasoning_effort"], "none");
}

#[test]
fn reasoning_on_unsupported_model_warns_and_omits_effort() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        reasoning: Some(ReasoningLevel::High),
        ..Default::default()
    };
    let (body, warnings) = model("grok-4.20-reasoning").get_args(&options).unwrap();
    assert!(body.get("reasoning_effort").is_none());
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, Warning::Unsupported { .. }))
    );
}

#[test]
fn explicit_reasoning_effort_option_wins_over_level() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        reasoning: Some(ReasoningLevel::High),
        provider_options: Some(xai_provider_options(serde_json::json!({
            "reasoningEffort": "none"
        }))),
        ..Default::default()
    };
    let (body, _) = model("grok-4.5").get_args(&options).unwrap();
    assert_eq!(body["reasoning_effort"], "none");
}

#[test]
fn response_format_json_object_without_schema() {
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        response_format: Some(ResponseFormat::json()),
        ..Default::default()
    };
    let (body, _) = model("grok-4.5").get_args(&options).unwrap();
    assert_eq!(body["response_format"]["type"], "json_object");
}

#[test]
fn response_format_json_schema_is_always_strict() {
    let schema = serde_json::json!({"type": "object", "properties": {}});
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::user_text("hi")],
        response_format: Some(ResponseFormat::json_with_schema(schema).with_name("out")),
        ..Default::default()
    };
    let (body, _) = model("grok-4.5").get_args(&options).unwrap();
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["response_format"]["json_schema"]["name"], "out");
    assert_eq!(body["response_format"]["json_schema"]["strict"], true);
}

#[test]
fn supported_urls_accepts_image_urls() {
    let m = model("grok-4.5");
    let urls = m.supported_urls();
    assert!(urls.contains_key("image/*"));
    let re = &urls["image/*"][0];
    assert!(re.is_match("https://example.com/a.png"));
    assert!(!re.is_match("ftp://example.com/a.png"));
}
