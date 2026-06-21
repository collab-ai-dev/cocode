use super::*;

#[test]
fn deserializes_anthropic_error() {
    let json = r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens: 0 is less than minimum of 1"}}"#;
    let data: AnthropicErrorData = serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        data.error.message,
        "max_tokens: 0 is less than minimum of 1"
    );
    assert_eq!(
        data.error.error_type.as_deref(),
        Some("invalid_request_error")
    );
}
