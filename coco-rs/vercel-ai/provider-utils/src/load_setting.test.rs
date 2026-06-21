use super::*;

#[test]
fn test_load_setting_direct() {
    let result: String = load_setting(Some("test".to_string()), "VAR", "default".to_string());
    assert_eq!(result, "test");
}

#[test]
fn test_load_setting_default() {
    let result: String = load_setting(None, "NONEXISTENT_VAR_12345", "default".to_string());
    assert_eq!(result, "default");
}

#[test]
fn test_load_bool_setting() {
    assert!(load_bool_setting(Some(true), "VAR", false));
    assert!(!load_bool_setting(Some(false), "VAR", true));
    assert!(!load_bool_setting(None, "NONEXISTENT_VAR", false));
}
