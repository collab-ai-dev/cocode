use super::*;

#[test]
fn test_search_input_state_insert() {
    let mut state = SearchInputState::new();
    state.insert('h');
    state.insert('e');
    state.insert('l');
    state.insert('l');
    state.insert('o');
    assert_eq!(state.query, "hello");
    assert_eq!(state.cursor, 5);
}

#[test]
fn test_search_input_state_backspace() {
    let mut state = SearchInputState::new();
    state.set_query("hello".to_string());
    state.backspace();
    assert_eq!(state.query, "hell");
    assert_eq!(state.cursor, 4);
}

#[test]
fn test_search_input_state_move() {
    let mut state = SearchInputState::new();
    state.set_query("hello".to_string());
    state.move_left();
    assert_eq!(state.cursor, 4);
    state.move_start();
    assert_eq!(state.cursor, 0);
    state.move_end();
    assert_eq!(state.cursor, 5);
}

#[test]
fn test_search_mode_cycle() {
    let mut state = SearchInputState::new();
    assert_eq!(state.mode, SearchMode::Hybrid);
    state.next_mode();
    assert_eq!(state.mode, SearchMode::Bm25);
    state.next_mode();
    assert_eq!(state.mode, SearchMode::Vector);
    state.prev_mode();
    assert_eq!(state.mode, SearchMode::Bm25);
}

#[test]
fn test_history_push() {
    let mut state = SearchInputState::new();
    state.set_query("first".to_string());
    state.push_history();
    state.set_query("second".to_string());
    state.push_history();

    assert_eq!(state.history.len(), 2);
    assert_eq!(state.history[0], "first");
    assert_eq!(state.history[1], "second");
}

#[test]
fn test_history_dedup() {
    let mut state = SearchInputState::new();
    state.set_query("query".to_string());
    state.push_history();
    state.set_query("query".to_string());
    state.push_history();

    // Duplicate should be removed, only one entry
    assert_eq!(state.history.len(), 1);
}

#[test]
fn test_history_navigation() {
    let mut state = SearchInputState::new();
    state.set_query("first".to_string());
    state.push_history();
    state.set_query("second".to_string());
    state.push_history();
    state.set_query("current".to_string());

    // Navigate to previous (second)
    state.prev_history();
    assert_eq!(state.query, "second");
    assert!(state.is_navigating_history());

    // Navigate to older (first)
    state.prev_history();
    assert_eq!(state.query, "first");

    // Navigate back to newer (second)
    state.next_history();
    assert_eq!(state.query, "second");

    // Navigate back to current
    state.next_history();
    assert_eq!(state.query, "current");
    assert!(!state.is_navigating_history());
}

#[test]
fn test_history_reset_on_type() {
    let mut state = SearchInputState::new();
    state.set_query("first".to_string());
    state.push_history();
    state.set_query("current".to_string());

    // Start navigating
    state.prev_history();
    assert!(state.is_navigating_history());

    // Type something - should reset navigation
    state.reset_history_navigation();
    assert!(!state.is_navigating_history());
}
