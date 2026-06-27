//! Tests for call_settings.rs

use super::*;

#[test]
fn test_timeout_configuration() {
    let timeout = TimeoutConfiguration::new()
        .with_total_ms(60000)
        .with_step_ms(30000)
        .with_chunk_ms(5000);

    assert_eq!(timeout.total_ms, Some(60000));
    assert_eq!(timeout.step_ms, Some(30000));
    assert_eq!(timeout.chunk_ms, Some(5000));
}

#[test]
fn test_call_settings_with_timeout() {
    let timeout = TimeoutConfiguration::new().with_total_ms(30000);
    let settings = CallSettings::new()
        .with_max_tokens(1000)
        .with_temperature(0.7)
        .with_timeout(timeout);

    assert_eq!(settings.max_tokens, Some(1000));
    assert_eq!(settings.temperature, Some(0.7));
    assert!(settings.timeout.is_some());
    let timeout = settings.timeout.unwrap();
    assert_eq!(timeout.total_ms, Some(30000));
}

#[test]
fn test_call_settings_with_max_retries() {
    let settings = CallSettings::new().with_max_retries(3);
    assert_eq!(settings.max_retries, Some(3));
}
