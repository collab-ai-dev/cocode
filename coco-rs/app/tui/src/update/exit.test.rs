use std::time::Duration;
use std::time::Instant;

use pretty_assertions::assert_eq;

use super::ExitEffect;
use super::on_interrupt;
use super::on_request_exit;
use crate::state::AppState;
use crate::transcript::derive::test_helpers;
use coco_tui_ui::constants::DOUBLE_PRESS_TIMEOUT;

fn idle_state_with_history() -> AppState {
    let mut s = AppState::new();
    test_helpers::push_user_text(&mut s.session, "u1", "hello");
    test_helpers::push_assistant_text(&mut s.session, "hi");
    s
}

fn fresh_idle_state() -> AppState {
    AppState::new()
}

// ── on_interrupt ────────────────────────────────────────────────

#[test]
fn busy_session_yields_interrupt_only_and_clears_pending() {
    let mut state = fresh_idle_state();
    let now = Instant::now();
    // First press in idle arms the tracker.
    let _ = on_interrupt(&mut state, now);
    assert!(state.ui.ctrl_c_tracker.pending().is_some());

    // Now mark busy and press again — the busy branch wins and clears
    // the previous arm (no dangling "press again" hint).
    state.session.set_busy(true);
    let later = now + Duration::from_millis(100);
    assert_eq!(on_interrupt(&mut state, later), ExitEffect::InterruptOnly);
    assert!(state.ui.ctrl_c_tracker.pending().is_none());
}

#[test]
fn streaming_session_routes_to_interrupt_only() {
    let mut state = fresh_idle_state();
    state.ui.streaming = Some(crate::state::StreamingState::default());
    assert_eq!(
        on_interrupt(&mut state, Instant::now()),
        ExitEffect::InterruptOnly,
    );
}

#[test]
fn queued_commands_route_to_interrupt_only() {
    use crate::state::QueuedCommandDisplay;
    let mut state = fresh_idle_state();
    state
        .session
        .queued_commands
        .push_back(QueuedCommandDisplay {
            id: "q1".into(),
            preview: "queued".into(),
            editable: true,
        });
    assert_eq!(
        on_interrupt(&mut state, Instant::now()),
        ExitEffect::InterruptOnly,
    );
}

#[test]
fn idle_first_press_arms_only() {
    // Idle Ctrl+C never opens rewind — it only arms the double-press exit
    // prompt. Auto-restore (if conditions match) is decided later by the
    // TurnInterrupted handler, not here.
    let mut state = fresh_idle_state();
    let effect = on_interrupt(&mut state, Instant::now());
    assert_eq!(effect, ExitEffect::ArmOnly);
    assert!(state.ui.ctrl_c_tracker.pending().is_some());
}

#[test]
fn idle_first_press_with_history_still_only_arms() {
    // Having previous user messages does NOT change the keypress-time
    // decision — the rewind picker is only reachable via double-Esc /
    // /rewind, and the in-place restore is the TurnInterrupted
    // handler's job.
    let mut state = idle_state_with_history();
    let effect = on_interrupt(&mut state, Instant::now());
    assert_eq!(effect, ExitEffect::ArmOnly);
    assert!(state.ui.ctrl_c_tracker.pending().is_some());
}

#[test]
fn idle_double_press_within_window_quits() {
    let mut state = fresh_idle_state();
    let t0 = Instant::now();
    on_interrupt(&mut state, t0);
    let t1 = t0 + Duration::from_millis(100);
    assert_eq!(on_interrupt(&mut state, t1), ExitEffect::Quit);
}

#[test]
fn idle_double_press_after_window_is_a_fresh_first() {
    let mut state = fresh_idle_state();
    let t0 = Instant::now();
    on_interrupt(&mut state, t0);
    let t1 = t0 + DOUBLE_PRESS_TIMEOUT + Duration::from_millis(1);
    let effect = on_interrupt(&mut state, t1);
    // Stale arm doesn't fire double — second press is itself a First.
    assert_ne!(effect, ExitEffect::Quit);
}

// ── on_request_exit (Ctrl+D) ────────────────────────────────────

#[test]
fn first_ctrl_d_press_arms_only() {
    let mut state = fresh_idle_state();
    let effect = on_request_exit(&mut state, Instant::now());
    assert_eq!(effect, ExitEffect::ArmOnly);
    assert!(state.ui.ctrl_d_tracker.pending().is_some());
}

#[test]
fn double_ctrl_d_within_window_quits() {
    let mut state = fresh_idle_state();
    let t0 = Instant::now();
    on_request_exit(&mut state, t0);
    let t1 = t0 + Duration::from_millis(100);
    assert_eq!(on_request_exit(&mut state, t1), ExitEffect::Quit);
}

#[test]
fn ctrl_d_does_not_interrupt_busy_session() {
    let mut state = fresh_idle_state();
    state.session.set_busy(true);
    // Ctrl+D is exit-only — never cancels work.
    let effect = on_request_exit(&mut state, Instant::now());
    assert_eq!(effect, ExitEffect::ArmOnly);
}

// ── Cross-key interaction ────────────────────────────────────────

#[test]
fn ctrl_c_then_ctrl_d_then_ctrl_c_within_window_quits_on_ctrl_c() {
    // Each tracker has its own counter, so pressing Ctrl+D between two
    // Ctrl+Cs does NOT cancel the Ctrl+C double-press counter.
    let mut state = fresh_idle_state();
    let t0 = Instant::now();
    on_interrupt(&mut state, t0); // arm Ctrl+C
    let t1 = t0 + Duration::from_millis(100);
    on_request_exit(&mut state, t1); // arm Ctrl+D (independent)
    let t2 = t1 + Duration::from_millis(100);
    assert_eq!(on_interrupt(&mut state, t2), ExitEffect::Quit);
}

#[test]
fn pending_hint_reflects_most_recently_armed_key() {
    let mut state = fresh_idle_state();
    let t0 = Instant::now();
    on_interrupt(&mut state, t0);
    assert_eq!(
        state.ui.pending_exit_hint(),
        Some(crate::state::ExitKey::CtrlC)
    );
    let t1 = t0 + Duration::from_millis(100);
    on_request_exit(&mut state, t1);
    assert_eq!(
        state.ui.pending_exit_hint(),
        Some(crate::state::ExitKey::CtrlD)
    );
}

#[test]
fn tick_clears_pending_hint_after_window() {
    let mut state = fresh_idle_state();
    let t0 = Instant::now();
    on_interrupt(&mut state, t0);
    assert!(state.ui.pending_exit_hint().is_some());
    let expired = state
        .ui
        .tick_double_press(t0 + DOUBLE_PRESS_TIMEOUT + Duration::from_millis(1));
    assert!(expired);
    assert!(state.ui.pending_exit_hint().is_none());
}
