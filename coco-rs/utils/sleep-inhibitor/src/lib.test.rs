use super::SleepInhibitor;

#[test]
fn sleep_inhibitor_toggles_without_panicking() {
    let mut inhibitor = SleepInhibitor::new(true);
    inhibitor.set_turn_running(true);
    assert!(inhibitor.is_turn_running());
    inhibitor.set_turn_running(false);
    assert!(!inhibitor.is_turn_running());
}

#[test]
fn sleep_inhibitor_disabled_does_not_panic() {
    let mut inhibitor = SleepInhibitor::new(false);
    inhibitor.set_turn_running(true);
    assert!(inhibitor.is_turn_running());
    inhibitor.set_turn_running(false);
    assert!(!inhibitor.is_turn_running());
}

#[test]
fn sleep_inhibitor_multiple_true_calls_are_idempotent() {
    let mut inhibitor = SleepInhibitor::new(true);
    inhibitor.set_turn_running(true);
    inhibitor.set_turn_running(true);
    inhibitor.set_turn_running(true);
    inhibitor.set_turn_running(false);
}

#[test]
fn sleep_inhibitor_can_toggle_multiple_times() {
    let mut inhibitor = SleepInhibitor::new(true);
    inhibitor.set_turn_running(true);
    inhibitor.set_turn_running(false);
    inhibitor.set_turn_running(true);
    inhibitor.set_turn_running(false);
}
