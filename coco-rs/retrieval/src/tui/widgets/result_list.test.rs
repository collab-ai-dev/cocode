use super::*;
use crate::types::ScoreType;

fn make_result(filepath: &str, score: f32, score_type: ScoreType) -> SearchResultSummary {
    SearchResultSummary {
        filepath: filepath.to_string(),
        start_line: 10,
        end_line: 20,
        score,
        score_type,
        language: "rust".to_string(),
        preview: Some("fn main() {}".to_string()),
        is_stale: None,
    }
}

#[test]
fn test_result_list_state_navigation() {
    let mut state = ResultListState::new();

    let results = vec![
        make_result("a.rs", 0.9, ScoreType::Bm25),
        make_result("b.rs", 0.8, ScoreType::Vector),
        make_result("c.rs", 0.7, ScoreType::Hybrid),
    ];
    state.set_results(results, 100);

    assert_eq!(state.selected_index(), Some(0));

    state.select_next();
    assert_eq!(state.selected_index(), Some(1));

    state.select_next();
    assert_eq!(state.selected_index(), Some(2));

    // Wrap around
    state.select_next();
    assert_eq!(state.selected_index(), Some(0));

    state.select_previous();
    assert_eq!(state.selected_index(), Some(2));
}

#[test]
fn test_result_list_state_page_navigation() {
    let mut state = ResultListState::new();

    let results: Vec<SearchResultSummary> = (0..25)
        .map(|i| {
            make_result(
                &format!("file{}.rs", i),
                1.0 - (i as f32 * 0.01),
                ScoreType::Hybrid,
            )
        })
        .collect();
    state.set_results(results, 200);

    state.page_down();
    assert_eq!(state.selected_index(), Some(10));

    state.page_down();
    assert_eq!(state.selected_index(), Some(20));

    state.page_down();
    assert_eq!(state.selected_index(), Some(24)); // Clamped to last

    state.page_up();
    assert_eq!(state.selected_index(), Some(14));

    state.select_first();
    assert_eq!(state.selected_index(), Some(0));

    state.select_last();
    assert_eq!(state.selected_index(), Some(24));
}

#[test]
fn test_start_search_clears_state() {
    let mut state = ResultListState::new();

    let results = vec![make_result("a.rs", 0.9, ScoreType::Bm25)];
    state.set_results(results, 100);
    assert!(!state.results.is_empty());
    assert!(state.duration_ms.is_some());

    state.start_search();
    assert!(state.results.is_empty());
    assert!(state.duration_ms.is_none());
    assert!(state.searching);
    assert!(state.selected_index().is_none());
}

#[test]
fn test_truncate_path_no_truncation() {
    // Short path should not be truncated
    assert_eq!(truncate_path("short.rs", 20), "short.rs");
    assert_eq!(truncate_path("a/b/c.rs", 20), "a/b/c.rs");
}

#[test]
fn test_truncate_path_with_truncation() {
    // Long path should be truncated with ellipsis
    let long_path = "very/long/path/to/deeply/nested/file.rs";
    let truncated = truncate_path(long_path, 25);
    assert!(truncated.starts_with('…'));
    assert!(truncated.ends_with("file.rs"));
    // Check display width, not byte length
    let display_width = UnicodeWidthStr::width(truncated.as_str());
    assert!(display_width <= 25);
}

#[test]
fn test_truncate_path_preserves_filename() {
    // Filename should always be preserved when possible
    let path = "a/b/c/d/e/f/g/h/i/very_long_filename.rs";
    let truncated = truncate_path(path, 30);
    assert!(truncated.contains("very_long_filename.rs"));
}

#[test]
fn test_truncate_path_small_width() {
    // Very small width should still work
    let path = "path/to/file.rs";
    let truncated = truncate_path(path, 3);
    // With max_width < 4, return original
    assert_eq!(truncated, path);
}
