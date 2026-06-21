use super::*;
use pretty_assertions::assert_eq;

#[test]
fn note_viewport_initializes_without_change() {
    let mut state = HistoryReflowState::default();

    assert_eq!(
        state.note_viewport(80),
        HistoryViewportChange {
            initialized: true,
            changed: false,
        }
    );
    assert!(!state.replay_needed_for_viewport(80));
}

#[test]
fn note_viewport_reports_changed_after_width_change() {
    let mut state = HistoryReflowState::default();
    state.note_viewport(80);

    assert_eq!(
        state.note_viewport(100),
        HistoryViewportChange {
            initialized: false,
            changed: true,
        }
    );
    assert!(state.replay_needed_for_viewport(100));
}

#[test]
fn note_viewport_ignores_height_only_change() {
    // Same width, different viewport height (the live tail growing during a
    // stream) must NOT report a change or schedule a reflow — scrollback rows
    // re-wrap only on width, so a height change needs no full-history replay.
    let mut state = HistoryReflowState::default();
    state.note_viewport(80);

    assert_eq!(
        state.note_viewport(80),
        HistoryViewportChange {
            initialized: false,
            changed: false,
        }
    );
    assert!(!state.replay_needed_for_viewport(80));
}

#[test]
fn scheduled_viewport_is_not_reported_as_needed_twice() {
    let mut state = HistoryReflowState::default();
    state.note_viewport(80);
    state.note_viewport(100);

    state.schedule_viewport_replay(100, false);

    assert_eq!(state.pending_viewport(), Some(100));
    assert!(!state.replay_needed_for_viewport(100));
}

#[test]
fn pending_replay_becomes_due_after_deadline() {
    let mut state = HistoryReflowState::default();
    state.schedule_viewport_replay(100, false);
    assert!(!state.pending_is_due(Instant::now()));

    state.force_due_for_test();

    assert!(state.pending_is_due(Instant::now()));
}

#[test]
fn mark_replayed_viewport_clears_pending_state() {
    let mut state = HistoryReflowState::default();
    state.note_viewport(80);
    state.schedule_viewport_replay(100, false);

    state.mark_replayed_viewport(100, false);

    assert_eq!(state.pending_viewport(), None);
    assert!(!state.replay_needed_for_viewport(100));
}

#[test]
fn clear_resets_reflow_state() {
    let mut state = HistoryReflowState::default();
    state.note_viewport(80);
    state.schedule_viewport_replay(100, true);

    state.clear();

    assert_eq!(state.pending_viewport(), None);
    assert_eq!(
        state.note_viewport(100),
        HistoryViewportChange {
            initialized: true,
            changed: false,
        }
    );
}
