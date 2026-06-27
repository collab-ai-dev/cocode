use super::*;

#[test]
fn test_fallback_trips_on_consecutive_threshold() {
    let mut tracker = DenialTracker::new();
    assert!(!tracker.should_fallback_to_prompting());

    tracker.record_denial("Bash");
    tracker.record_denial("Bash");
    assert!(!tracker.should_fallback_to_prompting());

    tracker.record_denial("Bash");
    assert!(tracker.should_fallback_to_prompting());
    assert!(tracker.is_stuck());
    assert!(!tracker.hit_total_limit());
}

#[test]
fn test_fallback_trips_on_total_threshold_without_consecutive() {
    let mut tracker = DenialTracker::new();
    // Interleave resets so the consecutive gate never fires, but total climbs.
    for _ in 0..20 {
        tracker.record_denial("Bash");
        tracker.reset_consecutive();
    }
    assert!(!tracker.is_stuck());
    assert!(tracker.hit_total_limit());
    assert!(tracker.should_fallback_to_prompting());
}

#[test]
fn test_reset_consecutive_clears_consecutive_gate() {
    let mut tracker = DenialTracker::new();
    for _ in 0..3 {
        tracker.record_denial("Bash");
    }
    assert!(tracker.should_fallback_to_prompting());

    tracker.reset_consecutive();
    assert!(!tracker.is_stuck());
    // Below the total cap, an allowed action clears the fallback condition.
    assert!(!tracker.should_fallback_to_prompting());
}

#[test]
fn test_reset_after_total_limit_clears_both() {
    let mut tracker = DenialTracker::new();
    for _ in 0..20 {
        tracker.record_denial("Bash");
        tracker.reset_consecutive();
    }
    assert!(tracker.should_fallback_to_prompting());
    tracker.reset_after_total_limit();
    assert_eq!(tracker.total_denials, 0);
    assert_eq!(tracker.consecutive_denials, 0);
    assert!(!tracker.should_fallback_to_prompting());
}

#[test]
fn test_per_tool_tracking() {
    let mut tracker = DenialTracker::new();
    tracker.record_denial("Bash");
    tracker.record_denial("Bash");
    tracker.record_denial("Write");

    assert_eq!(tracker.tool_denial_count("Bash"), 2);
    assert_eq!(tracker.tool_denial_count("Write"), 1);
    assert_eq!(tracker.most_denied_tool(), Some(("Bash", 2)));
}

#[test]
fn test_suggestion_message() {
    let mut tracker = DenialTracker::new();
    assert!(tracker.suggestion_message().is_none());

    for _ in 0..3 {
        tracker.record_denial("Bash");
    }
    let msg = tracker.suggestion_message().unwrap();
    assert!(msg.contains("consecutive"));
    assert!(msg.contains("Bash"));
}

#[test]
fn test_subagent_fork_isolation_no_parent_pollution() {
    // Regression: parent's denial count must not bleed into a fork.
    // Subagent context always builds a fresh DenialTracker.
    let mut parent = DenialTracker::new();
    let mut fork = DenialTracker::new();

    // Fork hits 3 denies and trips its own fallback condition.
    for _ in 0..3 {
        fork.record_denial("Bash");
    }
    assert!(fork.should_fallback_to_prompting());

    // Parent stays clean.
    assert_eq!(parent.consecutive_denials, 0);
    assert!(!parent.should_fallback_to_prompting());

    // Parent denials still trip independently.
    for _ in 0..3 {
        parent.record_denial("Write");
    }
    assert!(parent.should_fallback_to_prompting());
    assert_eq!(parent.tool_denial_count("Bash"), 0);
}
