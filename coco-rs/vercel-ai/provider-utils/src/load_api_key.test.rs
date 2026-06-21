use super::*;

#[test]
fn test_load_api_key_direct() {
    let result = load_api_key(Some("test-key"), "NONEXISTENT_VAR", "Test");
    assert_eq!(result.unwrap(), "test-key");
}

#[test]
fn test_load_api_key_empty_string_uses_env() {
    // Empty string should fall back to env
    let result = load_api_key(Some(""), "NONEXISTENT_VAR_12345", "Test");
    assert!(result.is_err());
}

#[test]
fn test_load_optional_api_key() {
    let result = load_optional_api_key(Some("test-key"), "NONEXISTENT_VAR");
    assert_eq!(result, Some("test-key".to_string()));

    let result = load_optional_api_key(None, "NONEXISTENT_VAR_12345");
    assert_eq!(result, None);
}
