use super::*;

#[test]
fn creates_provider_with_defaults() {
    let provider = OpenAICompatibleProvider::new(OpenAICompatibleProviderSettings {
        api_key: Some("sk-test".into()),
        ..Default::default()
    });
    assert_eq!(provider.provider(), "openai-compatible");
}

#[test]
fn creates_provider_with_custom_name() {
    let provider = OpenAICompatibleProvider::new(OpenAICompatibleProviderSettings {
        name: Some("xai".into()),
        api_key: Some("sk-test".into()),
        ..Default::default()
    });
    assert_eq!(provider.provider(), "xai");
}

#[test]
fn chat_model_has_correct_provider() {
    let provider = OpenAICompatibleProvider::new(OpenAICompatibleProviderSettings {
        name: Some("xai".into()),
        api_key: Some("sk-test".into()),
        ..Default::default()
    });
    let model = provider.chat("grok-2");
    assert_eq!(model.model_id(), "grok-2");
    assert_eq!(model.provider(), "xai.chat");
}

#[test]
fn language_model_defaults_to_chat() {
    let provider = OpenAICompatibleProvider::new(OpenAICompatibleProviderSettings {
        name: Some("xai".into()),
        api_key: Some("sk-test".into()),
        ..Default::default()
    });
    let model = provider.language_model("grok-2").expect("should succeed");
    // Unlike OpenAI which defaults to Responses, openai-compatible defaults to Chat
    assert_eq!(model.provider(), "xai.chat");
}

#[test]
fn embedding_model_works() {
    let provider = OpenAICompatibleProvider::new(OpenAICompatibleProviderSettings {
        name: Some("xai".into()),
        api_key: Some("sk-test".into()),
        ..Default::default()
    });
    let model = provider
        .embedding_model("text-embedding-3-small")
        .expect("should succeed");
    assert_eq!(model.model_id(), "text-embedding-3-small");
}

#[test]
fn image_model_works() {
    let provider = OpenAICompatibleProvider::new(OpenAICompatibleProviderSettings {
        name: Some("xai".into()),
        api_key: Some("sk-test".into()),
        ..Default::default()
    });
    let model = provider.image_model("dall-e-3").expect("should succeed");
    assert_eq!(model.model_id(), "dall-e-3");
}

#[test]
fn provider_with_query_params() {
    let provider = OpenAICompatibleProvider::new(OpenAICompatibleProviderSettings {
        name: Some("azure".into()),
        api_key: Some("sk-test".into()),
        base_url: Some("https://my-deployment.openai.azure.com/openai".into()),
        query_params: Some(HashMap::from([("api-version".into(), "2024-02-01".into())])),
        ..Default::default()
    });
    assert_eq!(provider.provider(), "azure");
}

#[test]
fn provider_with_custom_env_var() {
    let provider = OpenAICompatibleProvider::new(OpenAICompatibleProviderSettings {
        name: Some("groq".into()),
        api_key: Some("gsk-test".into()),
        api_key_env_var: Some("GROQ_API_KEY".into()),
        api_key_description: Some("Groq".into()),
        ..Default::default()
    });
    assert_eq!(provider.provider(), "groq");
}

#[test]
fn deepseek_profile_defaults_stream_usage_on() {
    let provider = OpenAICompatibleProvider::new(OpenAICompatibleProviderSettings {
        name: Some("deepseek-openai".into()),
        api_key: Some("sk-test".into()),
        provider_profile: Some(OpenAICompatibleProviderProfile::DeepSeek),
        ..Default::default()
    });
    assert!(provider.include_usage);
}

#[test]
fn generic_profile_preserves_stream_usage_default_off() {
    let provider = OpenAICompatibleProvider::new(OpenAICompatibleProviderSettings {
        api_key: Some("sk-test".into()),
        ..Default::default()
    });
    assert!(!provider.include_usage);
}
