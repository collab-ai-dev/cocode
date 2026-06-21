use super::*;

#[test]
fn gemini_model_detection() {
    let model = GoogleGenerativeAIImageModel::new(
        "gemini-2.0-flash",
        GoogleGenerativeAIImageSettings::default(),
        GoogleGenerativeAIImageModelConfig {
            provider: "google.generative-ai".to_string(),
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            headers: Arc::new(HashMap::new),
            client: None,
        },
    );
    assert!(model.is_gemini_model());
    assert_eq!(model.max_images_per_call(), 10);
}

#[test]
fn imagen_model_detection() {
    let model = GoogleGenerativeAIImageModel::new(
        "imagen-3.0-generate-002",
        GoogleGenerativeAIImageSettings::default(),
        GoogleGenerativeAIImageModelConfig {
            provider: "google.generative-ai".to_string(),
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            headers: Arc::new(HashMap::new),
            client: None,
        },
    );
    assert!(!model.is_gemini_model());
    assert_eq!(model.max_images_per_call(), 4);
}

#[test]
fn custom_max_images() {
    let model = GoogleGenerativeAIImageModel::new(
        "gemini-2.0-flash",
        GoogleGenerativeAIImageSettings {
            max_images_per_call: Some(2),
        },
        GoogleGenerativeAIImageModelConfig {
            provider: "google.generative-ai".to_string(),
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            headers: Arc::new(HashMap::new),
            client: None,
        },
    );
    assert_eq!(model.max_images_per_call(), 2);
}

#[test]
fn model_id_and_provider() {
    let model = GoogleGenerativeAIImageModel::new(
        "imagen-3.0-generate-002",
        GoogleGenerativeAIImageSettings::default(),
        GoogleGenerativeAIImageModelConfig {
            provider: "google.generative-ai".to_string(),
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            headers: Arc::new(HashMap::new),
            client: None,
        },
    );
    assert_eq!(model.model_id(), "imagen-3.0-generate-002");
    assert_eq!(model.provider(), "google.generative-ai");
}

#[test]
fn gemini_model_prefix_detection() {
    // "gemini-" prefix models are Gemini
    let model = GoogleGenerativeAIImageModel::new(
        "gemini-2.0-flash",
        GoogleGenerativeAIImageSettings::default(),
        GoogleGenerativeAIImageModelConfig {
            provider: "test".to_string(),
            base_url: "https://test".to_string(),
            headers: Arc::new(HashMap::new),
            client: None,
        },
    );
    assert!(model.is_gemini_model());

    // "gemini" without dash is not Gemini (matching TS behavior)
    let model2 = GoogleGenerativeAIImageModel::new(
        "gemini_other",
        GoogleGenerativeAIImageSettings::default(),
        GoogleGenerativeAIImageModelConfig {
            provider: "test".to_string(),
            base_url: "https://test".to_string(),
            headers: Arc::new(HashMap::new),
            client: None,
        },
    );
    assert!(!model2.is_gemini_model());
}
