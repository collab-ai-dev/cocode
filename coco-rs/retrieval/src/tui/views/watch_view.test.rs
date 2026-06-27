use super::*;

#[test]
fn test_watch_view_not_watching() {
    let event_log = EventLogState::new();
    let _view = WatchView::new(false, &event_log);
}

#[test]
fn test_watch_view_watching() {
    let event_log = EventLogState::new();
    let _view = WatchView::new(true, &event_log);
}
