use std::collections::HashMap;

use vercel_ai_provider::ProviderV4;

use super::*;

#[test]
fn creates_provider_with_defaults() {
    let provider = bytedance();
    assert_eq!(provider.provider_name, "bytedance");
    assert_eq!(provider.base_url, DEFAULT_BASE_URL);
}

#[test]
fn creates_provider_with_custom_settings() {
    let provider = create_bytedance(ByteDanceProviderSettings {
        base_url: Some("https://custom.api.com/".to_string()),
        api_key: Some("test-key".to_string()),
        name: Some("custom-provider".to_string()),
        ..Default::default()
    });
    assert_eq!(provider.provider_name, "custom-provider");
    // Trailing slash should be stripped
    assert_eq!(provider.base_url, "https://custom.api.com");
}

#[test]
fn builds_headers_with_api_key() {
    let provider = create_bytedance(ByteDanceProviderSettings {
        api_key: Some("test-api-key".to_string()),
        ..Default::default()
    });
    let headers = provider.build_headers().unwrap();
    assert_eq!(headers.get("authorization").unwrap(), "Bearer test-api-key");
    assert_eq!(headers.get("content-type").unwrap(), "application/json");
}

#[test]
fn builds_headers_with_extra_headers() {
    let mut extra = HashMap::new();
    extra.insert("x-custom".to_string(), "custom-value".to_string());

    let provider = create_bytedance(ByteDanceProviderSettings {
        api_key: Some("test-key".to_string()),
        headers: Some(extra),
        ..Default::default()
    });
    let headers = provider.build_headers().unwrap();
    assert_eq!(headers.get("x-custom").unwrap(), "custom-value");
    assert_eq!(headers.get("authorization").unwrap(), "Bearer test-key");
}

#[test]
fn creates_video_model_instance() {
    let provider = create_bytedance(ByteDanceProviderSettings {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let model = provider
        .video_model_instance("seedance-1-5-pro-251215")
        .unwrap();
    assert_eq!(model.model_id(), "seedance-1-5-pro-251215");
    assert_eq!(model.provider(), "bytedance");
}

#[test]
fn video_model_via_trait() {
    let provider = create_bytedance(ByteDanceProviderSettings {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let model = provider.video_model("seedance-1-5-pro-251215").unwrap();
    assert_eq!(model.model_id(), "seedance-1-5-pro-251215");
    assert_eq!(model.provider(), "bytedance");
}

#[test]
fn language_model_returns_error() {
    let provider = create_bytedance(ByteDanceProviderSettings {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let result = provider.language_model("some-model");
    let err = result.err().expect("should be an error");
    assert!(
        err.to_string().contains("does not support language models"),
        "Error message was: {err}",
    );
}

#[test]
fn embedding_model_returns_error() {
    let provider = create_bytedance(ByteDanceProviderSettings {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let result = provider.embedding_model("some-model");
    let err = result.err().expect("should be an error");
    assert!(
        err.to_string()
            .contains("does not support embedding models"),
        "Error message was: {err}",
    );
}

#[test]
fn image_model_returns_error() {
    let provider = create_bytedance(ByteDanceProviderSettings {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let result = provider.image_model("some-model");
    let err = result.err().expect("should be an error");
    assert!(
        err.to_string().contains("does not support image models"),
        "Error message was: {err}",
    );
}
