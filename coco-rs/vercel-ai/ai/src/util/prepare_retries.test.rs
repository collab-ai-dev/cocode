use super::*;

#[test]
fn test_retry_settings_default() {
    let settings = RetrySettings::new();

    assert_eq!(settings.max_retries, 3);
    assert_eq!(settings.initial_delay, Duration::from_millis(100));
    assert_eq!(settings.max_delay, Duration::from_secs(60));
    assert_eq!(settings.backoff_multiplier, 2.0);
}

#[test]
fn test_retry_settings_builder() {
    let settings = RetrySettings::new()
        .with_max_retries(5)
        .with_initial_delay(Duration::from_millis(200))
        .with_max_delay(Duration::from_secs(30))
        .with_backoff_multiplier(1.5);

    assert_eq!(settings.max_retries, 5);
    assert_eq!(settings.initial_delay, Duration::from_millis(200));
    assert_eq!(settings.max_delay, Duration::from_secs(30));
    assert_eq!(settings.backoff_multiplier, 1.5);
}

#[test]
fn test_calculate_delay() {
    let settings = RetrySettings::new()
        .with_initial_delay(Duration::from_millis(100))
        .with_backoff_multiplier(2.0);

    assert_eq!(settings.calculate_delay(0), Duration::from_millis(100));
    assert_eq!(settings.calculate_delay(1), Duration::from_millis(200));
    assert_eq!(settings.calculate_delay(2), Duration::from_millis(400));
}

#[test]
fn test_calculate_delay_max() {
    let settings = RetrySettings::new()
        .with_initial_delay(Duration::from_millis(1000))
        .with_max_delay(Duration::from_millis(5000))
        .with_backoff_multiplier(10.0);

    // Should cap at max_delay
    assert_eq!(settings.calculate_delay(2), Duration::from_millis(5000));
}

#[test]
fn test_is_retryable_status() {
    let settings = RetrySettings::new();

    assert!(settings.is_retryable_status(429));
    assert!(settings.is_retryable_status(500));
    assert!(settings.is_retryable_status(503));
    assert!(!settings.is_retryable_status(400));
    assert!(!settings.is_retryable_status(401));
}

#[test]
fn test_is_exhausted() {
    let settings = RetrySettings::new().with_max_retries(3);

    assert!(!settings.is_exhausted(0));
    assert!(!settings.is_exhausted(2));
    assert!(settings.is_exhausted(3));
    assert!(settings.is_exhausted(4));
}

#[test]
fn test_prepare_retries() {
    let settings = prepare_retries(Some(5), Some(500));

    assert_eq!(settings.max_retries, 5);
    assert_eq!(settings.initial_delay, Duration::from_millis(500));
}

#[test]
fn test_prepare_provider_retries_anthropic() {
    let settings = prepare_provider_retries("anthropic");

    assert_eq!(settings.max_retries, 2);
    assert!(settings.is_retryable_status(529));
}

#[test]
fn test_prepare_provider_retries_openai() {
    let settings = prepare_provider_retries("openai");

    assert_eq!(settings.max_retries, 3);
}
