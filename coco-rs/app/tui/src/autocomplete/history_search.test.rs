use super::*;
use crate::state::HistoryEntry;

fn history(texts: &[&str]) -> Vec<HistoryEntry> {
    texts
        .iter()
        .map(|t| HistoryEntry {
            text: (*t).to_string(),
            timestamp_ms: None,
            pastes: Vec::new(),
        })
        .collect()
}

fn labels(results: &HistoryResults) -> Vec<String> {
    results.items.iter().map(|i| i.label.clone()).collect()
}

#[test]
fn empty_query_browses_newest_first() {
    // index 0 = newest.
    let h = history(&["newest", "middle", "oldest"]);
    let results = search_history(&h, "");
    assert_eq!(labels(&results), vec!["newest", "middle", "oldest"]);
    assert_eq!(results.indices, vec![0, 1, 2]);
    // No query → no highlights.
    assert!(results.items.iter().all(|i| i.highlight_indices.is_empty()));
}

#[test]
fn fuzzy_query_ranks_and_reports_indices() {
    let h = history(&["git status", "git commit --amend", "cargo test -p coco-tui"]);
    let results = search_history(&h, "gcm");
    // "git commit --amend" is the strongest subsequence match for g-c-m.
    assert_eq!(
        results.items.first().map(|i| i.label.as_str()),
        Some("git commit --amend")
    );
    // The winning row carries per-char highlight indices.
    assert!(!results.items[0].highlight_indices.is_empty());
}

#[test]
fn non_matching_query_returns_no_results() {
    let h = history(&["alpha", "beta"]);
    let results = search_history(&h, "zzzzz");
    assert!(results.items.is_empty());
    assert!(results.indices.is_empty());
}

#[test]
fn results_are_capped() {
    let texts: Vec<String> = (0..40).map(|i| format!("command number {i}")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let h = history(&refs);
    let results = search_history(&h, "command");
    assert!(results.items.len() <= MAX_HISTORY_RESULTS);
    assert_eq!(results.items.len(), results.indices.len());
}

#[test]
fn multiline_entry_label_is_first_line_with_ellipsis() {
    let h = history(&["first line\nsecond line"]);
    let results = search_history(&h, "");
    assert_eq!(results.items[0].label, "first line …");
}

#[test]
fn multiline_entry_does_not_match_invisible_later_line() {
    let h = history(&["visible title\nhidden needle"]);
    let results = search_history(&h, "needle");
    assert!(results.items.is_empty());
}
