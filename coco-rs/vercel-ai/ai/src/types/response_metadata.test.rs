//! Tests for response_metadata.rs

use super::*;

#[test]
fn test_request_metadata() {
    let meta =
        LanguageModelRequestMetadata::new().with_body(serde_json::json!({ "model": "gpt-4" }));

    assert!(meta.body.is_some());
    assert_eq!(meta.body.unwrap()["model"], "gpt-4");
}

#[test]
fn test_response_metadata() {
    let mut headers = HashMap::new();
    headers.insert("x-request-id".to_string(), "123".to_string());

    let ts = chrono::DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let meta = LanguageModelResponseMetadata::new()
        .with_id("resp-123")
        .with_timestamp(ts)
        .with_model_id("gpt-4")
        .with_headers(headers);

    assert_eq!(meta.id, Some("resp-123".to_string()));
    assert_eq!(meta.timestamp, Some(ts));
    assert_eq!(meta.model_id, Some("gpt-4".to_string()));
    assert!(meta.headers.is_some());
}
