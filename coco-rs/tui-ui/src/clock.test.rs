use super::Clock;
use super::MockClock;
use super::SystemClock;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn system_clock_returns_close_to_real_time() {
    let c = SystemClock;
    let now_ms = c.now_ms();
    // Sanity: should be in the 2026+ ballpark — sometime past
    // 2025-01-01 (1735689600_000), well before 2200.
    assert!(
        now_ms > 1_735_689_600_000,
        "implausibly old now_ms: {now_ms}"
    );
    assert!(
        now_ms < 7_258_118_400_000,
        "implausibly future now_ms: {now_ms}"
    );
}

#[test]
fn mock_clock_is_pinned_to_now_ms() {
    let c = MockClock::new(1_000_000_000);
    assert_eq!(c.now_ms(), 1_000_000_000);
    assert_eq!(c.now_ms(), 1_000_000_000, "now_ms is stable across calls");
}

#[test]
fn mock_clock_advance_shifts_now_ms() {
    let c = MockClock::new(1_000_000_000);
    c.advance(5_000);
    assert_eq!(c.now_ms(), 1_000_005_000);
    c.advance(-2_000);
    assert_eq!(c.now_ms(), 1_000_003_000);
}

#[test]
fn mock_clock_advance_shifts_instant_consistently() {
    let c = MockClock::new(1_000_000_000);
    let i0 = c.now();
    c.advance(750);
    let i1 = c.now();
    let delta = i1.saturating_duration_since(i0);
    assert_eq!(delta, Duration::from_millis(750));
}

#[test]
fn mock_clock_arc_is_dyn_compatible() {
    // The whole point of the Arc<dyn Clock> shape is that
    // production AppState can hold a SystemClock arc while tests
    // hold a MockClock arc through the same trait object.
    let arc: Arc<dyn Clock> = MockClock::arc(42);
    assert_eq!(arc.now_ms(), 42);
}

#[test]
fn mock_clock_handles_negative_offsets_without_panic() {
    let c = MockClock::new(1_000_000_000);
    c.advance(-100);
    // `now()` must subtract correctly from base_instant without
    // panicking on the unsigned-cast path.
    let _ = c.now();
}
