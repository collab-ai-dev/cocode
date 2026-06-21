use super::*;

#[test]
fn test_invalid_response_data_error_new() {
    let data = serde_json::json!({"error": "invalid"});
    let error = InvalidResponseDataError::new(data.clone());
    assert_eq!(error.data, data);
    assert!(error.message.contains("Invalid response data"));
}

#[test]
fn test_invalid_response_data_error_with_message() {
    let data = serde_json::json!({"code": 500});
    let error = InvalidResponseDataError::with_message(data.clone(), "Server error");
    assert_eq!(error.data, data);
    assert_eq!(error.message, "Server error");
}

#[test]
fn test_invalid_response_data_error_display() {
    let data = serde_json::json!({"foo": "bar"});
    let error = InvalidResponseDataError::with_message(data, "Custom message");
    assert_eq!(format!("{error}"), "Custom message");
}
