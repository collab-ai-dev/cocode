use super::UiEphemeralState;
use pretty_assertions::assert_eq;
use std::time::Duration;
use std::time::Instant;

#[test]
fn elapsed_is_zero_when_no_turn() {
    let s = UiEphemeralState::new();
    assert_eq!(s.elapsed_ms(Instant::now()), 0);
}

#[test]
fn elapsed_grows_with_wall_clock_when_unpaused() {
    let t0 = Instant::now();
    let mut s = UiEphemeralState::new();
    s.start_turn("Pondering", t0);
    assert_eq!(s.elapsed_ms(t0 + Duration::from_millis(750)), 750);
}

#[test]
fn tick_pauses_when_prompt_appears() {
    let t0 = Instant::now();
    let mut s = UiEphemeralState::new();
    s.start_turn("Pondering", t0);
    s.tick_pause_clock(/*blocked*/ true, t0 + Duration::from_millis(200));
    let turn = s.turn.as_ref().expect("turn running");
    assert!(turn.pause_started_at.is_some());
    assert_eq!(turn.total_paused_ms, 0);
    // Displayed elapsed freezes at the pause-start instant (200ms).
    let displayed = s.elapsed_ms(t0 + Duration::from_millis(800));
    assert_eq!(displayed, 200, "elapsed must freeze at pause start");
}

#[test]
fn tick_accumulates_pause_on_resume() {
    let t0 = Instant::now();
    let mut s = UiEphemeralState::new();
    s.start_turn("Pondering", t0);
    s.tick_pause_clock(true, t0 + Duration::from_millis(200));
    s.tick_pause_clock(false, t0 + Duration::from_millis(800));
    let turn = s.turn.as_ref().expect("turn running");
    assert!(turn.pause_started_at.is_none());
    assert_eq!(turn.total_paused_ms, 600);
    let displayed = s.elapsed_ms(t0 + Duration::from_millis(1000));
    assert_eq!(displayed, 400);
}

#[test]
fn tick_handles_back_to_back_pause_intervals() {
    let t0 = Instant::now();
    let mut s = UiEphemeralState::new();
    s.start_turn("Pondering", t0);
    s.tick_pause_clock(true, t0 + Duration::from_millis(100));
    s.tick_pause_clock(false, t0 + Duration::from_millis(300));
    s.tick_pause_clock(true, t0 + Duration::from_millis(500));
    s.tick_pause_clock(false, t0 + Duration::from_millis(900));
    // Two pauses: 200ms + 400ms = 600ms total paused.
    assert_eq!(s.turn.as_ref().unwrap().total_paused_ms, 600);
    let displayed = s.elapsed_ms(t0 + Duration::from_millis(1000));
    assert_eq!(displayed, 400);
}

#[test]
fn tick_is_idempotent_for_stale_state() {
    let t0 = Instant::now();
    let mut s = UiEphemeralState::new();
    s.start_turn("Pondering", t0);
    s.tick_pause_clock(true, t0 + Duration::from_millis(100));
    let anchor = s.turn.as_ref().unwrap().pause_started_at;
    s.tick_pause_clock(true, t0 + Duration::from_millis(500));
    assert_eq!(
        s.turn.as_ref().unwrap().pause_started_at,
        anchor,
        "stale paused→paused must not move the anchor"
    );
    s.tick_pause_clock(false, t0 + Duration::from_millis(900));
    s.tick_pause_clock(false, t0 + Duration::from_millis(1000));
    assert_eq!(s.turn.as_ref().unwrap().total_paused_ms, 800);
}

#[test]
fn tick_is_noop_when_no_turn() {
    let t0 = Instant::now();
    let mut s = UiEphemeralState::new();
    // No start_turn — no anchor.
    s.tick_pause_clock(true, t0 + Duration::from_millis(100));
    assert!(
        s.turn.is_none(),
        "tick must not lazily create a turn — no-op when idle"
    );
}

#[test]
fn end_turn_preserves_total_paused_ms() {
    let t0 = Instant::now();
    let mut s = UiEphemeralState::new();
    s.start_turn("Pondering", t0);
    s.tick_pause_clock(true, t0 + Duration::from_millis(100));
    s.tick_pause_clock(false, t0 + Duration::from_millis(500));
    assert_eq!(s.turn.as_ref().unwrap().total_paused_ms, 400);

    s.end_turn();
    assert!(s.turn.is_none(), "end_turn drops the running-turn record");
    assert_eq!(
        s.last_total_paused_ms, 400,
        "final total_paused_ms preserved for stalled-frame renders"
    );
}

#[test]
fn start_turn_resets_accumulators_and_samples_verb() {
    let t0 = Instant::now();
    let mut s = UiEphemeralState::new();
    // Simulate a prior turn that left state behind.
    s.start_turn("StaleVerb", t0 - Duration::from_secs(10));
    s.tick_pause_clock(true, t0 - Duration::from_secs(9));
    s.end_turn();
    assert_eq!(s.last_total_paused_ms, 0); // pause never closed; accumulator was 0
    s.last_total_paused_ms = 1234; // simulate non-zero residual

    s.start_turn("Pondering", t0);
    let turn = s.turn.as_ref().expect("turn running");
    assert_eq!(turn.started_at, t0);
    assert_eq!(turn.verb, "Pondering");
    assert_eq!(turn.total_paused_ms, 0);
    assert!(turn.pause_started_at.is_none());
    assert_eq!(
        s.last_total_paused_ms, 0,
        "start_turn must clear the residual so a fresh elapsed clock isn't double-subtracted"
    );
}

#[test]
fn helpers_reflect_turn_state() {
    let t0 = Instant::now();
    let mut s = UiEphemeralState::new();

    assert!(!s.turn_active());
    assert_eq!(s.current_verb(), None);
    assert_eq!(s.turn_started_at(), None);

    s.start_turn("Pondering", t0);
    assert!(s.turn_active());
    assert_eq!(s.current_verb(), Some("Pondering"));
    assert_eq!(s.turn_started_at(), Some(t0));

    s.end_turn();
    assert!(!s.turn_active());
    assert_eq!(s.current_verb(), None);
    assert_eq!(s.turn_started_at(), None);
}

#[test]
fn mark_interrupting_is_noop_without_a_turn() {
    let mut s = UiEphemeralState::new();
    s.mark_interrupting();
    assert!(!s.is_interrupting(), "no turn → nothing to interrupt");
}

#[test]
fn mark_interrupting_flags_running_turn_and_clears_on_end() {
    let t0 = Instant::now();
    let mut s = UiEphemeralState::new();

    s.start_turn("Pondering", t0);
    assert!(!s.is_interrupting(), "fresh turn is not interrupting");

    s.mark_interrupting();
    assert!(s.is_interrupting());
    // Idempotent: a second Esc/Ctrl+C must not regress the flag.
    s.mark_interrupting();
    assert!(s.is_interrupting());
    // The verb itself is untouched — the renderer substitutes the
    // "Interrupting…" label; the sampled verb stays for any fallback.
    assert_eq!(s.current_verb(), Some("Pondering"));

    // Terminal event takes the whole RunningTurn, so the flag is gone.
    s.end_turn();
    assert!(!s.is_interrupting(), "end_turn clears the interrupt flag");

    // A subsequent turn starts clean.
    s.start_turn("Pondering", t0);
    assert!(!s.is_interrupting(), "next turn must not inherit the flag");
}
