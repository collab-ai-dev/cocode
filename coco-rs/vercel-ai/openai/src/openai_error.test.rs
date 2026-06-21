use super::*;

#[test]
fn deserialize_error_response() {
    let json = r#"{"error":{"message":"Invalid API key","type":"invalid_request_error","param":null,"code":"invalid_api_key"}}"#;
    let data: OpenAIErrorData = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(data.error.message, "Invalid API key");
    assert_eq!(
        data.error.error_type.as_deref(),
        Some("invalid_request_error")
    );
}
