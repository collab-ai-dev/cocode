use super::*;

#[test]
fn test_response_metadata_new() {
    let meta = LanguageModelV4ResponseMetadata::new();
    assert!(meta.id.is_none());
    assert!(meta.timestamp.is_none());
    assert!(meta.model_id.is_none());
}

#[test]
fn test_response_metadata_default() {
    let meta = LanguageModelV4ResponseMetadata::default();
    assert!(meta.id.is_none());
}

#[test]
fn test_response_metadata_with_id() {
    let meta = LanguageModelV4ResponseMetadata::new().with_id("resp-123");
    assert_eq!(meta.id, Some("resp-123".to_string()));
}

#[test]
fn test_response_metadata_with_timestamp() {
    let meta = LanguageModelV4ResponseMetadata::new().with_timestamp("2024-01-01T00:00:00Z");
    assert_eq!(meta.timestamp, Some("2024-01-01T00:00:00Z".to_string()));
}

#[test]
fn test_response_metadata_with_model_id() {
    let meta = LanguageModelV4ResponseMetadata::new().with_model_id("gpt-4");
    assert_eq!(meta.model_id, Some("gpt-4".to_string()));
}

#[test]
fn test_response_metadata_builder_chain() {
    let meta = LanguageModelV4ResponseMetadata::new()
        .with_id("resp-1")
        .with_timestamp("2024-01-01T00:00:00Z")
        .with_model_id("gpt-4");
    assert_eq!(meta.id, Some("resp-1".to_string()));
    assert_eq!(meta.timestamp, Some("2024-01-01T00:00:00Z".to_string()));
    assert_eq!(meta.model_id, Some("gpt-4".to_string()));
}

#[test]
fn test_response_metadata_serialization() {
    let meta = LanguageModelV4ResponseMetadata::new()
        .with_id("resp-1")
        .with_model_id("gpt-4");
    let json = serde_json::to_string(&meta).unwrap();
    assert!(json.contains(r#""id":"resp-1"#));
    assert!(json.contains(r#""modelId":"gpt-4"#));
    // timestamp should not be serialized when None
    assert!(!json.contains("timestamp"));
}
