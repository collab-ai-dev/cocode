use super::*;

#[test]
fn model_id_and_provider() {
    let model = GoogleGenerativeAIVideoModel::new(
        "veo-2.0-generate-001",
        GoogleGenerativeAIVideoModelConfig {
            provider: "google.generative-ai".to_string(),
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            headers: Arc::new(HashMap::new),
            client: None,
        },
    );
    assert_eq!(model.model_id(), "veo-2.0-generate-001");
    assert_eq!(model.provider(), "google.generative-ai");
}

#[test]
fn default_poll_settings() {
    let opts: Option<GoogleGenerativeAIVideoSettings> = None;
    assert_eq!(
        GoogleGenerativeAIVideoModel::poll_interval(&opts),
        Duration::from_secs(10)
    );
    assert_eq!(
        GoogleGenerativeAIVideoModel::poll_timeout(&opts),
        Duration::from_secs(600)
    );
}

#[test]
fn custom_poll_settings() {
    let opts = Some(GoogleGenerativeAIVideoSettings {
        poll_interval_ms: Some(5000),
        poll_timeout_ms: Some(120_000),
        ..Default::default()
    });
    assert_eq!(
        GoogleGenerativeAIVideoModel::poll_interval(&opts),
        Duration::from_secs(5)
    );
    assert_eq!(
        GoogleGenerativeAIVideoModel::poll_timeout(&opts),
        Duration::from_secs(120)
    );
}

#[test]
fn resolution_mapping() {
    assert_eq!(
        GoogleGenerativeAIVideoModel::map_resolution("1280x720"),
        "720p"
    );
    assert_eq!(
        GoogleGenerativeAIVideoModel::map_resolution("1920x1080"),
        "1080p"
    );
    assert_eq!(
        GoogleGenerativeAIVideoModel::map_resolution("3840x2160"),
        "4k"
    );
    assert_eq!(
        GoogleGenerativeAIVideoModel::map_resolution("custom"),
        "custom"
    );
}
