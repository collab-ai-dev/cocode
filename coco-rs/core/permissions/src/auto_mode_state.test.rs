use super::*;

#[test]
fn test_default_inactive() {
    let state = AutoModeState::new();
    assert!(!state.is_active());
    assert!(!state.cli_flag());
    assert!(!state.is_circuit_broken());
    assert!(state.is_gate_enabled());
}

#[test]
fn test_set_active() {
    let state = AutoModeState::new();
    state.set_active(true);
    assert!(state.is_active());
    state.set_active(false);
    assert!(!state.is_active());
}

#[test]
fn test_circuit_breaker_disables_gate() {
    let state = AutoModeState::new();
    assert!(state.is_gate_enabled());
    state.set_circuit_broken(true);
    assert!(!state.is_gate_enabled());
}

#[test]
fn test_cli_flag() {
    let state = AutoModeState::new();
    state.set_cli_flag(true);
    assert!(state.cli_flag());
}

#[test]
fn test_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AutoModeState>();
}
