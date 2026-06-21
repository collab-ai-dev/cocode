use super::*;

#[test]
fn creates_provider_with_defaults() {
    let provider = google();
    assert_eq!(provider.provider_name, "google.generative-ai");
    assert_eq!(provider.base_url, DEFAULT_BASE_URL);
}

#[test]
fn creates_provider_with_custom_settings() {
    let provider = create_google_generative_ai(GoogleGenerativeAIProviderSettings {
        base_url: Some("https://custom.api.com/".to_string()),
        api_key: Some("test-key".to_string()),
        name: Some("custom-provider".to_string()),
        headers: None,
    });
    assert_eq!(provider.provider_name, "custom-provider");
    // Trailing slash should be stripped
    assert_eq!(provider.base_url, "https://custom.api.com");
}

#[test]
fn builds_headers_with_api_key() {
    let provider = create_google_generative_ai(GoogleGenerativeAIProviderSettings {
        api_key: Some("test-api-key".to_string()),
        ..Default::default()
    });
    let headers = provider.build_headers().unwrap();
    assert_eq!(headers.get("x-goog-api-key").unwrap(), "test-api-key");
    assert_eq!(headers.get("content-type").unwrap(), "application/json");
}

#[test]
fn builds_headers_with_extra_headers() {
    let mut extra = HashMap::new();
    extra.insert("x-custom".to_string(), "custom-value".to_string());

    let provider = create_google_generative_ai(GoogleGenerativeAIProviderSettings {
        api_key: Some("test-key".to_string()),
        headers: Some(extra),
        ..Default::default()
    });
    let headers = provider.build_headers().unwrap();
    assert_eq!(headers.get("x-custom").unwrap(), "custom-value");
    assert_eq!(headers.get("x-goog-api-key").unwrap(), "test-key");
}

#[test]
fn creates_language_model_instance() {
    let provider = create_google_generative_ai(GoogleGenerativeAIProviderSettings {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let model = provider
        .language_model_instance("gemini-2.0-flash")
        .unwrap();
    assert_eq!(model.model_id(), "gemini-2.0-flash");
    assert_eq!(model.provider(), "google.generative-ai");
}

#[test]
fn creates_embedding_model_instance() {
    let provider = create_google_generative_ai(GoogleGenerativeAIProviderSettings {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let model = provider
        .embedding_model_instance("text-embedding-004")
        .unwrap();
    assert_eq!(model.model_id(), "text-embedding-004");
}

#[test]
fn creates_image_model_instance() {
    let provider = create_google_generative_ai(GoogleGenerativeAIProviderSettings {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let model = provider
        .image_model_instance("imagen-3.0-generate-002")
        .unwrap();
    assert_eq!(model.model_id(), "imagen-3.0-generate-002");
}

#[test]
fn creates_video_model_instance() {
    let provider = create_google_generative_ai(GoogleGenerativeAIProviderSettings {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let model = provider
        .video_model_instance("veo-2.0-generate-001")
        .unwrap();
    assert_eq!(model.model_id(), "veo-2.0-generate-001");
}

#[test]
fn provider_v4_language_model() {
    let provider = create_google_generative_ai(GoogleGenerativeAIProviderSettings {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    });
    let model = provider.language_model("gemini-2.0-flash").unwrap();
    assert_eq!(model.model_id(), "gemini-2.0-flash");
    assert_eq!(model.provider(), "google.generative-ai");
}
