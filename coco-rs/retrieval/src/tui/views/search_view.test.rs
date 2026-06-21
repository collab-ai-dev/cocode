use super::*;

#[test]
fn test_search_view_creation() {
    let search = SearchState::default();
    let event_log = EventLogState::new();
    let _view = SearchView::new(&search, &event_log);
}
