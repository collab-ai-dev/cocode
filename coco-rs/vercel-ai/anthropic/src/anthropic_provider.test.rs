use super::*;

#[test]
fn creates_provider_with_defaults() {
    let provider = AnthropicProvider::new(AnthropicProviderSettings {
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    assert_eq!(provider.provider(), "anthropic.messages");
}

#[test]
fn creates_provider_with_custom_name() {
    let provider = AnthropicProvider::new(AnthropicProviderSettings {
        api_key: Some("test-key".into()),
        name: Some("custom.anthropic".into()),
        ..Default::default()
    });
    assert_eq!(provider.provider(), "custom.anthropic");
}

#[test]
fn creates_provider_with_custom_base_url() {
    let provider = AnthropicProvider::new(AnthropicProviderSettings {
        api_key: Some("test-key".into()),
        base_url: Some("https://custom.api.com/v1".into()),
        ..Default::default()
    });
    assert_eq!(provider.base_url, "https://custom.api.com/v1");
}

#[test]
fn language_model_returns_messages_model() {
    let provider = AnthropicProvider::new(AnthropicProviderSettings {
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let model = provider
        .language_model("claude-sonnet-4-5")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(model.model_id(), "claude-sonnet-4-5");
    assert_eq!(model.provider(), "anthropic.messages");
}

#[test]
fn embedding_model_returns_error() {
    let provider = AnthropicProvider::new(AnthropicProviderSettings {
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let result = provider.embedding_model("text-embedding-3-small");
    assert!(result.is_err());
}

#[test]
fn image_model_returns_error() {
    let provider = AnthropicProvider::new(AnthropicProviderSettings {
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let result = provider.image_model("dall-e-3");
    assert!(result.is_err());
}

#[test]
fn headers_include_anthropic_version() {
    let provider = AnthropicProvider::new(AnthropicProviderSettings {
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    let config = provider.make_config();
    let headers = config.get_headers();
    assert_eq!(
        headers.get("anthropic-version").map(String::as_str),
        Some("2023-06-01")
    );
}

#[test]
fn headers_use_x_api_key_by_default() {
    let provider = AnthropicProvider::new(AnthropicProviderSettings {
        api_key: Some("sk-ant-123".into()),
        ..Default::default()
    });
    let config = provider.make_config();
    let headers = config.get_headers();
    assert_eq!(
        headers.get("x-api-key").map(String::as_str),
        Some("sk-ant-123")
    );
    assert!(!headers.contains_key("Authorization"));
}

#[test]
fn headers_use_bearer_when_auth_token_provided() {
    let provider = AnthropicProvider::new(AnthropicProviderSettings {
        auth_token: Some("my-token".into()),
        ..Default::default()
    });
    let config = provider.make_config();
    let headers = config.get_headers();
    assert_eq!(
        headers.get("Authorization").map(String::as_str),
        Some("Bearer my-token")
    );
    assert!(!headers.contains_key("x-api-key"));
}

#[test]
fn custom_headers_override_defaults() {
    let mut custom = HashMap::new();
    custom.insert("anthropic-version".into(), "2024-01-01".into());
    let provider = AnthropicProvider::new(AnthropicProviderSettings {
        api_key: Some("test-key".into()),
        headers: Some(custom),
        ..Default::default()
    });
    let config = provider.make_config();
    let headers = config.get_headers();
    assert_eq!(
        headers.get("anthropic-version").map(String::as_str),
        Some("2024-01-01")
    );
}
