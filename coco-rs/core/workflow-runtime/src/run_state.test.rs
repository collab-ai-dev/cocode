use pretty_assertions::assert_eq;

use super::WorkflowRunState;

#[test]
fn test_next_agent_index_is_zero_based_and_monotonic() {
    let state = WorkflowRunState::default();
    assert_eq!(
        (0..4).map(|_| state.next_agent_index()).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
}

#[test]
fn test_resolve_phase_interns_by_exact_title() {
    let state = WorkflowRunState::default();

    let first = state.resolve_phase("Scan");
    assert_eq!((first.index, first.is_new), (1, true));

    let again = state.resolve_phase("Scan");
    assert_eq!((again.index, again.is_new), (1, false));

    // Exact match only — whitespace makes a distinct phase.
    let padded = state.resolve_phase("Scan ");
    assert_eq!((padded.index, padded.is_new), (2, true));
}

#[test]
fn test_seed_phases_take_indices_before_any_agent_runs() {
    let state = WorkflowRunState::new(["Scan", "Fix", "Scan", ""]);

    // Duplicates and blanks are dropped; declared order is preserved.
    assert_eq!(state.resolve_phase("Scan").index, 1);
    assert_eq!(state.resolve_phase("Fix").index, 2);
    assert!(!state.resolve_phase("Scan").is_new);

    // An undeclared phase lands after the declared ones.
    let extra = state.resolve_phase("Verify");
    assert_eq!((extra.index, extra.is_new), (3, true));
}

#[test]
fn test_replay_divergence_is_a_one_way_latch() {
    let state = WorkflowRunState::default();
    assert!(state.replay_allowed());
    state.mark_replay_diverged();
    assert!(!state.replay_allowed());
    state.mark_replay_diverged();
    assert!(!state.replay_allowed());
}
