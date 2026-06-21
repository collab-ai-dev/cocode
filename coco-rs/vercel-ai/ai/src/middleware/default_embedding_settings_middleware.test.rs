use super::*;

#[test]
fn test_middleware_creation() {
    let settings = DefaultEmbeddingSettings {
        headers: Some(std::collections::HashMap::from([(
            "X-Custom".to_string(),
            "value".to_string(),
        )])),
    };

    let _middleware = default_embedding_settings_middleware(settings);
}
