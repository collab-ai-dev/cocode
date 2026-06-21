use super::*;

#[test]
fn test_reload_tracker_initial_state() {
    let tracker = PluginReloadTracker::new();
    assert!(!tracker.take_reload_needed());
}

#[test]
fn test_reload_tracker_request_reload() {
    let tracker = PluginReloadTracker::new();
    tracker.request_reload();
    assert!(tracker.take_reload_needed());
    assert!(!tracker.take_reload_needed()); // consumed
}

#[test]
fn test_reload_tracker_snapshot_change_detection() {
    let tracker = PluginReloadTracker::new();
    assert!(tracker.update_snapshot("v1"));
    assert!(!tracker.update_snapshot("v1")); // same
    assert!(tracker.update_snapshot("v2")); // changed
}
