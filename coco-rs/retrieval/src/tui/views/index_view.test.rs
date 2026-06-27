use super::*;

#[test]
fn test_index_view_creation() {
    let index = IndexState::default();
    let event_log = EventLogState::new();
    let _view = IndexView::new(&index, &event_log);
}
