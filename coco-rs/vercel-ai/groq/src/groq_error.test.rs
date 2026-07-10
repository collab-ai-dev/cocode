use super::*;
use pretty_assertions::assert_eq;

#[test]
fn deserialize_error_response() {
    let json = r#"{"error":{"message":"Invalid API Key","type":"invalid_request_error"}}"#;
    let data: GroqErrorData = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(data.error.message, "Invalid API Key");
    assert_eq!(
        data.error.error_type.as_deref(),
        Some("invalid_request_error")
    );
}

#[test]
fn deserialize_error_without_type() {
    let json = r#"{"error":{"message":"Something went wrong"}}"#;
    let data: GroqErrorData = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(data.error.message, "Something went wrong");
    assert_eq!(data.error.error_type, None);
}
