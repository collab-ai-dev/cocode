use super::*;
use coco_config::AutoCompactConfig;

fn auto_default() -> AutoCompactConfig {
    AutoCompactConfig::default()
}

// --- should_reactive_compact tests ---

#[test]
fn test_should_reactive_compact() {
    let config = ReactiveCompactConfig::default(); // 95% of effective window
    let auto = auto_default();
    let effective = crate::auto_trigger::effective_context_window(
        config.context_window,
        config.max_output_tokens,
        &auto,
    );
    let reactive_threshold = effective * 95 / 100;
    assert!(!should_reactive_compact(
        reactive_threshold - 1000,
        &config,
        &auto
    ));
    assert!(should_reactive_compact(
        reactive_threshold + 1000,
        &config,
        &auto
    ));
}

#[test]
fn test_calculate_drop_target() {
    let config = ReactiveCompactConfig {
        context_window: 200_000,
        ..Default::default()
    };
    let drop = calculate_drop_target(195_000, &config, &auto_default());
    assert!(drop > 0);
    assert!(drop < 195_000);
}

#[test]
fn test_below_threshold_no_compact() {
    let config = ReactiveCompactConfig {
        context_window: 200_000,
        ..Default::default()
    };
    assert!(!should_reactive_compact(100_000, &config, &auto_default()));
}

// --- ReactiveCompactState / circuit breaker tests ---

#[test]
fn test_circuit_breaker_initially_open() {
    let state = ReactiveCompactState::new();
    assert!(
        state.should_attempt_reactive_compact(),
        "fresh state should allow compaction"
    );
    assert_eq!(state.failure_count(), 0);
}

#[test]
fn test_circuit_breaker_trips_after_threshold() {
    let mut state = ReactiveCompactState::new();
    state.record_failure(1000);
    assert!(state.should_attempt_reactive_compact(), "1 failure: ok");
    state.record_failure(2000);
    assert!(state.should_attempt_reactive_compact(), "2 failures: ok");
    state.record_failure(3000);
    assert!(
        !state.should_attempt_reactive_compact(),
        "3 failures: circuit breaker should trip"
    );
    assert_eq!(state.failure_count(), 3);
    assert_eq!(state.last_attempt_ms(), 3000);
}

#[test]
fn test_circuit_breaker_reset_on_success() {
    let mut state = ReactiveCompactState::new();
    state.record_failure(1000);
    state.record_failure(2000);
    assert_eq!(state.failure_count(), 2);

    state.record_success(3000);
    assert_eq!(state.failure_count(), 0);
    assert!(
        state.should_attempt_reactive_compact(),
        "success should reset circuit breaker"
    );
    assert_eq!(state.last_attempt_ms(), 3000);
}

#[test]
fn test_circuit_breaker_reset() {
    let mut state = ReactiveCompactState::new();
    state.record_failure(1000);
    state.record_failure(2000);
    state.record_failure(3000);
    assert!(!state.should_attempt_reactive_compact());

    state.reset();
    assert!(
        state.should_attempt_reactive_compact(),
        "reset should re-enable compaction"
    );
    assert_eq!(state.failure_count(), 0);
    assert_eq!(state.last_attempt_ms(), 0);
}

#[test]
fn test_circuit_breaker_failure_after_success_restarts_count() {
    let mut state = ReactiveCompactState::new();
    state.record_failure(1000);
    state.record_failure(2000);
    state.record_success(3000);
    assert_eq!(state.failure_count(), 0);

    state.record_failure(4000);
    assert_eq!(state.failure_count(), 1);
    assert!(
        state.should_attempt_reactive_compact(),
        "single failure after reset should be ok"
    );
}
