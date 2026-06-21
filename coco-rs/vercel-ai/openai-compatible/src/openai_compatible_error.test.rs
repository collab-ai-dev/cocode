use super::*;

#[test]
fn deserialize_error_response() {
    let json = r#"{"error":{"message":"Invalid API key","type":"invalid_request_error","param":null,"code":"invalid_api_key"}}"#;
    let data: OpenAICompatibleErrorData = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(data.error.message, "Invalid API key");
    assert_eq!(
        data.error.error_type.as_deref(),
        Some("invalid_request_error")
    );
}

#[test]
fn deserialize_error_with_numeric_code() {
    let json = r#"{"error":{"message":"Rate limit exceeded","type":"rate_limit","code":429}}"#;
    let data: OpenAICompatibleErrorData = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(data.error.message, "Rate limit exceeded");
    assert_eq!(data.error.code, Some(Value::Number(429.into())));
}
