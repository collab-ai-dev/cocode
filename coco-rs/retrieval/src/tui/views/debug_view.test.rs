use super::*;

#[test]
fn test_debug_view_creation() {
    let event_log = EventLogState::new();
    let _view = DebugView::new(&event_log);
}
