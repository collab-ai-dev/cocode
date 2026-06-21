use super::super::openai_completion_api::OpenAICompletionChoice;
use super::*;

fn make_config() -> Arc<OpenAIConfig> {
    Arc::new(OpenAIConfig {
        provider: "openai.completion".into(),
        base_url: "https://api.openai.com/v1".into(),
        headers: Arc::new(|| {
            let mut h = std::collections::HashMap::new();
            h.insert("Authorization".into(), "Bearer test".into());
            h
        }),
        client: None,
        full_url: None,
        chatgpt_subscription: false,
        reasoning_store: Default::default(),
    })
}

#[test]
fn creates_model() {
    let model = OpenAICompletionLanguageModel::new("gpt-3.5-turbo-instruct", make_config());
    assert_eq!(model.model_id(), "gpt-3.5-turbo-instruct");
    assert_eq!(model.provider(), "openai.completion");
}

#[test]
fn response_format_text_no_warning() {
    let options = LanguageModelV4CallOptions {
        response_format: Some(ResponseFormat::Text),
        ..Default::default()
    };
    let warnings = collect_completion_warnings(&options);
    assert!(
        !warnings.iter().any(
            |w| matches!(w, Warning::Unsupported { feature, .. } if feature == "responseFormat")
        ),
        "Text response format should not produce a warning"
    );
}

#[test]
fn response_format_json_no_schema_warns() {
    let options = LanguageModelV4CallOptions {
        response_format: Some(ResponseFormat::Json {
            schema: None,
            name: None,
            description: None,
        }),
        ..Default::default()
    };
    let warnings = collect_completion_warnings(&options);
    assert!(
        warnings.iter().any(
            |w| matches!(w, Warning::Unsupported { feature, .. } if feature == "responseFormat")
        ),
        "JSON format without schema should still produce a warning"
    );
}

#[test]
fn response_format_json_with_schema_warns() {
    let options = LanguageModelV4CallOptions {
        response_format: Some(ResponseFormat::Json {
            schema: Some(json!({"type": "object"})),
            name: None,
            description: None,
        }),
        ..Default::default()
    };
    let warnings = collect_completion_warnings(&options);
    assert!(
        warnings.iter().any(
            |w| matches!(w, Warning::Unsupported { feature, .. } if feature == "responseFormat")
        ),
        "JSON format with schema should produce a warning"
    );
}

#[test]
fn provider_metadata_always_present() {
    let response = OpenAICompletionResponse {
        id: Some("test".into()),
        model: Some("gpt-3.5-turbo-instruct".into()),
        created: Some(1700000000),
        choices: vec![OpenAICompletionChoice {
            text: Some("hello".into()),
            index: Some(0),
            finish_reason: Some("stop".into()),
            logprobs: None,
        }],
        usage: None,
    };
    let meta = build_completion_provider_metadata(&response);
    let openai = meta.0.get("openai").expect("should have openai key");
    assert!(openai.is_object(), "openai key should be an object");
}

#[test]
fn provider_metadata_includes_logprobs_when_present() {
    let response = OpenAICompletionResponse {
        id: Some("test".into()),
        model: Some("gpt-3.5-turbo-instruct".into()),
        created: Some(1700000000),
        choices: vec![OpenAICompletionChoice {
            text: Some("hello".into()),
            index: Some(0),
            finish_reason: Some("stop".into()),
            logprobs: Some(json!({"tokens": ["hello"]})),
        }],
        usage: None,
    };
    let meta = build_completion_provider_metadata(&response);
    let openai = meta.0.get("openai").expect("should have openai key");
    assert!(
        openai.get("logprobs").is_some(),
        "should include logprobs when present"
    );
}
