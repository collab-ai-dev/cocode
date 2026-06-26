use super::Clock;
use super::SystemClock;
use super::TestClock;

#[test]
fn system_clock_reads_advance() {
    let clock = SystemClock;
    let a = clock.now();
    let b = clock.now();
    assert!(b >= a, "monotonic now() must not go backwards");
    assert!(
        clock.now_unix_millis() > 0,
        "epoch millis should be positive"
    );
}

#[test]
fn test_clock_is_pinned_until_advanced() {
    let clock = TestClock::new(1_000);
    assert_eq!(clock.now_unix_millis(), 1_000);
    // now() is frozen relative to construction until advanced.
    let t0 = clock.now();
    assert_eq!(clock.now(), t0);

    clock.advance_millis(500);
    assert_eq!(clock.now_unix_millis(), 1_500);
    assert_eq!(clock.now() - t0, std::time::Duration::from_millis(500));
}

#[test]
fn test_clock_advances_backwards_too() {
    let clock = TestClock::new(10_000);
    clock.advance_millis(-3_000);
    assert_eq!(clock.now_unix_millis(), 7_000);
}

#[test]
fn test_clock_behaves_as_dyn_clock() {
    // Confirms the injection shape used by production code (`&dyn Clock`).
    fn reads(clock: &dyn Clock) -> i64 {
        clock.now_unix_millis()
    }
    let clock = TestClock::new(42);
    assert_eq!(reads(&clock), 42);
    assert!(reads(&SystemClock) > 0);
}
