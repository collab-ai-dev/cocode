use coco_types::ModelRole;
use pretty_assertions::assert_eq;

use super::render_context_usage;
use crate::state::AppState;
use crate::state::session::ModelBinding;
use crate::transcript::derive::test_helpers;

fn state_with_window(context_window: i64) -> AppState {
    let mut state = AppState::new();
    state.session.model_by_role.insert(
        ModelRole::Main,
        ModelBinding {
            model_id: "test-model".into(),
            provider: "test".into(),
            context_window: Some(context_window),
            effort: None,
        },
    );
    state
}

/// A compaction boundary anchors `ctx %` immediately. Before the boundary
/// became an anchor, a freshly compacted transcript carried no
/// assistant-usage anchor and the status bar showed `ctx --`.
#[test]
fn compact_boundary_anchors_context_usage() {
    let mut state = state_with_window(1_000_000);
    test_helpers::push_compact_boundary(
        &mut state.session,
        /*tokens_before*/ 800_000,
        /*tokens_after*/ 200_000,
    );

    let usage = render_context_usage(&state).expect("boundary provides an anchor");
    assert_eq!(usage.used, 200_000);
    assert_eq!(usage.total, 1_000_000);
    assert_eq!(usage.percent, 20);
}

/// The most recent anchor wins: a boundary after an earlier assistant turn
/// uses the boundary's `tokens_after`, not the stale pre-compact assistant.
#[test]
fn latest_anchor_wins_when_boundary_follows_assistant() {
    let mut state = state_with_window(1_000_000);
    test_helpers::push_assistant_text(&mut state.session, "pre-compact reply");
    test_helpers::push_compact_boundary(
        &mut state.session,
        /*tokens_before*/ 800_000,
        /*tokens_after*/ 200_000,
    );

    let usage = render_context_usage(&state).expect("boundary provides an anchor");
    // Nothing follows the boundary, so `used` is exactly `tokens_after`.
    assert_eq!(usage.used, 200_000);
}

/// No assistant-usage and no boundary ⇒ `None` ⇒ the status bar keeps
/// showing `ctx --` (unchanged behavior for a user-only transcript).
#[test]
fn no_anchor_yields_none() {
    let mut state = state_with_window(1_000_000);
    test_helpers::push_user_text(&mut state.session, "u1", "hello");

    assert!(render_context_usage(&state).is_none());
}
