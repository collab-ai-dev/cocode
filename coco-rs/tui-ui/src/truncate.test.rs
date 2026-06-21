use pretty_assertions::assert_eq;

use super::display_width;
use super::truncate_to_width;

#[test]
fn test_display_width_counts_wide_chars_as_two() {
    assert_eq!(display_width("hello"), 5);
    assert_eq!(display_width("中"), 2);
    assert_eq!(display_width("中文字"), 6);
}

#[test]
fn test_truncate_to_width_returns_short_input_unchanged() {
    assert_eq!(truncate_to_width("hello", 10), "hello");
    assert_eq!(truncate_to_width("hello", 5), "hello");
}

#[test]
fn test_truncate_to_width_appends_ellipsis_within_budget() {
    let out = truncate_to_width("hello world", 5);
    assert_eq!(out, "hell…");
    assert_eq!(display_width(&out), 5);
}

#[test]
fn test_truncate_to_width_respects_wide_grapheme_boundary() {
    // budget 4 cols (5 - ellipsis): two width-2 chars fit, third would overflow.
    let out = truncate_to_width("中文字", 5);
    assert_eq!(out, "中文…");
    assert_eq!(display_width(&out), 5);
}

#[test]
fn test_truncate_to_width_zero_is_empty() {
    assert_eq!(truncate_to_width("hello", 0), "");
}

#[test]
fn test_truncate_to_width_one_column_is_ellipsis_only() {
    assert_eq!(truncate_to_width("hello", 1), "…");
}
