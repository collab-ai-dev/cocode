use super::*;

#[test]
fn creates_provider_with_defaults() {
    let provider = OpenAIProvider::new(OpenAIProviderSettings {
        auth: crate::OpenAIAuth::ApiKey(Some("sk-test".into())),
        ..Default::default()
    });
    assert_eq!(provider.provider(), "openai");
}

#[test]
fn creates_provider_with_custom_name() {
    let provider = OpenAIProvider::new(OpenAIProviderSettings {
        name: Some("my-openai".into()),
        auth: crate::OpenAIAuth::ApiKey(Some("sk-test".into())),
        ..Default::default()
    });
    assert_eq!(provider.provider(), "my-openai");
}

#[test]
fn chat_model_has_correct_provider() {
    let provider = OpenAIProvider::new(OpenAIProviderSettings {
        auth: crate::OpenAIAuth::ApiKey(Some("sk-test".into())),
        ..Default::default()
    });
    let model = provider.chat("gpt-4o");
    assert_eq!(model.model_id(), "gpt-4o");
    assert_eq!(model.provider(), "openai.chat");
}

#[test]
fn responses_model_has_correct_provider() {
    let provider = OpenAIProvider::new(OpenAIProviderSettings {
        auth: crate::OpenAIAuth::ApiKey(Some("sk-test".into())),
        ..Default::default()
    });
    let model = provider.responses("gpt-4o");
    assert_eq!(model.model_id(), "gpt-4o");
    assert_eq!(model.provider(), "openai.responses");
}

#[test]
fn language_model_defaults_to_responses() {
    let provider = OpenAIProvider::new(OpenAIProviderSettings {
        auth: crate::OpenAIAuth::ApiKey(Some("sk-test".into())),
        ..Default::default()
    });
    let model = provider.language_model("gpt-4o").expect("should succeed");
    assert_eq!(model.provider(), "openai.responses");
}

#[test]
fn embedding_model_works() {
    let provider = OpenAIProvider::new(OpenAIProviderSettings {
        auth: crate::OpenAIAuth::ApiKey(Some("sk-test".into())),
        ..Default::default()
    });
    let model = provider
        .embedding_model("text-embedding-3-small")
        .expect("should succeed");
    assert_eq!(model.model_id(), "text-embedding-3-small");
}

#[test]
fn image_model_works() {
    let provider = OpenAIProvider::new(OpenAIProviderSettings {
        auth: crate::OpenAIAuth::ApiKey(Some("sk-test".into())),
        ..Default::default()
    });
    let model = provider.image_model("dall-e-3").expect("should succeed");
    assert_eq!(model.model_id(), "dall-e-3");
}
