use super::*;

#[test]
fn test_load_setting_error_new() {
    let error = LoadSettingError::new("API_KEY not found");
    assert_eq!(error.message, "API_KEY not found");
}

#[test]
fn test_load_setting_error_display() {
    let error = LoadSettingError::new("Missing configuration");
    assert_eq!(
        format!("{error}"),
        "Load setting error: Missing configuration"
    );
}

#[test]
fn test_load_setting_error_debug() {
    let error = LoadSettingError::new("test");
    let debug_str = format!("{error:?}");
    assert!(debug_str.contains("LoadSettingError"));
}
