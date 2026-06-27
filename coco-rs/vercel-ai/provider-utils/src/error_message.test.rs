use super::*;
use serde_json::json;

#[test]
fn test_get_error_message_error_message() {
    let json = json!({ "error": { "message": "test error" } });
    assert_eq!(get_error_message(&json), "test error");
}

#[test]
fn test_get_error_message_error_string() {
    let json = json!({ "error": "test error" });
    assert_eq!(get_error_message(&json), "test error");
}

#[test]
fn test_get_error_message_message() {
    let json = json!({ "message": "test error" });
    assert_eq!(get_error_message(&json), "test error");
}

#[test]
fn test_get_error_message_detail() {
    let json = json!({ "detail": "test error" });
    assert_eq!(get_error_message(&json), "test error");
}

#[test]
fn test_get_error_message_fallback() {
    let json = json!({ "data": "something" });
    assert!(get_error_message(&json).contains("data"));
}

#[test]
fn test_get_error_code() {
    let json = json!({ "error": { "code": "ERR001" } });
    assert_eq!(get_error_code(&json), Some("ERR001".to_string()));

    let json = json!({ "code": "ERR002" });
    assert_eq!(get_error_code(&json), Some("ERR002".to_string()));
}
