//! Tests for telemetry_settings.rs

use super::*;

#[test]
fn test_default_settings() {
    let settings = TelemetrySettings::default();
    assert!(!settings.is_enabled());
    assert!(settings.should_record_inputs());
    assert!(settings.should_record_outputs());
}

#[test]
fn test_builder_pattern() {
    let settings = TelemetrySettings::new()
        .with_enabled(true)
        .with_record_inputs(false)
        .with_record_outputs(false)
        .with_function_id("test-function");

    assert!(settings.is_enabled());
    assert!(!settings.should_record_inputs());
    assert!(!settings.should_record_outputs());
    assert_eq!(settings.function_id, Some("test-function".to_string()));
}
