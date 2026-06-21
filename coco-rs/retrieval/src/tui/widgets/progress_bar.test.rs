use super::*;

#[test]
fn test_progress_bar_state_start() {
    let mut state = ProgressBarState::new();
    state.start(100);

    assert!(state.in_progress);
    assert_eq!(state.total_items, 100);
    assert_eq!(state.progress, 0.0);
}

#[test]
fn test_progress_bar_state_update() {
    let mut state = ProgressBarState::new();
    state.start(100);
    state.update(IndexPhaseInfo::Chunking, 0.5, "Chunking files...");

    assert_eq!(state.phase, Some(IndexPhaseInfo::Chunking));
    assert_eq!(state.progress, 0.5);
    assert_eq!(state.description, "Chunking files...");
}

#[test]
fn test_progress_bar_state_complete() {
    let mut state = ProgressBarState::new();
    state.start(100);
    state.complete(5000);

    assert!(!state.in_progress);
    assert_eq!(state.progress, 1.0);
    assert_eq!(state.elapsed_ms, 5000);
}

#[test]
fn test_progress_bar_state_fail() {
    let mut state = ProgressBarState::new();
    state.start(100);
    state.fail("Disk full".to_string());

    assert!(!state.in_progress);
    assert_eq!(state.error, Some("Disk full".to_string()));
}
