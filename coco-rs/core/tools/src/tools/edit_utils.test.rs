use pretty_assertions::assert_eq;

use super::*;

#[test]
fn test_normalize_quotes_straight() {
    assert_eq!(normalize_quotes("hello"), "hello");
}

#[test]
fn test_normalize_quotes_curly_double() {
    let input = "\u{201C}hello\u{201D}";
    assert_eq!(normalize_quotes(input), "\"hello\"");
}

#[test]
fn test_normalize_quotes_curly_single() {
    let input = "\u{2018}hello\u{2019}";
    assert_eq!(normalize_quotes(input), "'hello'");
}

#[test]
fn test_find_actual_string_exact() {
    let content = "fn main() {}";
    assert_eq!(find_actual_string(content, "main"), Some("main"));
}

#[test]
fn test_find_actual_string_curly_quotes() {
    let content = "let x = \u{201C}hello\u{201D};";
    assert_eq!(
        find_actual_string(content, "\"hello\""),
        Some("\u{201C}hello\u{201D}")
    );
}

#[test]
fn test_find_actual_string_not_found() {
    assert_eq!(find_actual_string("abc", "xyz"), None);
}

#[test]
fn test_apply_edit_replace_once() {
    assert_eq!(apply_edit_to_file("aaa", "a", "b", false), "baa");
}

#[test]
fn test_apply_edit_replace_all() {
    assert_eq!(apply_edit_to_file("aaa", "a", "b", true), "bbb");
}

#[test]
fn test_apply_edit_deletion_strips_trailing_newline() {
    assert_eq!(apply_edit_to_file("foo\nbar\n", "foo", "", false), "bar\n");
}

#[test]
fn test_apply_edits_sequence() {
    let edits = vec![
        FileEdit {
            old_string: "foo".into(),
            new_string: "bar".into(),
            replace_all: false,
        },
        FileEdit {
            old_string: "baz".into(),
            new_string: "qux".into(),
            replace_all: false,
        },
    ];
    assert_eq!(apply_edits("foo baz", &edits), Ok("bar qux".into()));
}

#[test]
fn test_apply_edits_not_found() {
    let edits = vec![FileEdit {
        old_string: "xyz".into(),
        new_string: "abc".into(),
        replace_all: false,
    }];
    assert_eq!(apply_edits("hello", &edits), Err(EditError::StringNotFound));
}

#[test]
fn test_desanitize_for_edit_no_change() {
    let (old, new, changed) = desanitize_for_edit("hello", "world", "hello world");
    assert_eq!(old, "hello");
    assert_eq!(new, "world");
    assert!(!changed);
}

#[test]
fn test_desanitize_for_edit_with_match() {
    let file = "<name>foo</name>";
    let (old, new, changed) = desanitize_for_edit("<n>foo</n>", "<n>bar</n>", file);
    assert_eq!(old, "<name>foo</name>");
    assert_eq!(new, "<name>bar</name>");
    assert!(changed);
}

#[test]
fn test_strip_trailing_whitespace() {
    assert_eq!(strip_trailing_whitespace("foo  \nbar  \n"), "foo\nbar\n");
}

// ── closest_match_hint ──────────────────────────────────────────────

#[test]
fn test_closest_match_hint_near_miss_shows_numbered_context() {
    let content = "fn alpha() {}\n\nfn process_data(input: &str) -> String {\n    input.to_uppercase()\n}\n\nfn omega() {}\n";
    // Indent/name drift beyond exact match, but clearly similar.
    let old_string = "fn process_data(input: &str) -> Str {";
    let hint = closest_match_hint(old_string, content).expect("hint expected");
    assert!(hint.contains("   3| fn process_data(input: &str) -> String {"));
    // Two context lines around the anchor line.
    assert!(hint.contains("   1| fn alpha() {}"));
    assert!(hint.contains("   5| }"));
}

#[test]
fn test_closest_match_hint_nothing_similar_returns_none() {
    let content = "alpha\nbeta\ngamma\n";
    let old_string = "completely_unrelated_needle_zzzzqqqq(x, y, z)";
    assert_eq!(closest_match_hint(old_string, content), None);
}

#[test]
fn test_closest_match_hint_anchor_is_first_nonblank_line() {
    let content = "line one\ntarget_line_here\nline three\n";
    // Leading blank lines must not break anchoring.
    let old_string = "\n\ntarget_line_hore\nrest ignored";
    let hint = closest_match_hint(old_string, content).expect("hint expected");
    assert!(hint.contains("   2| target_line_here"));
}

#[test]
fn test_closest_match_hint_caps_at_three_candidates() {
    let content = "needle_a1\nneedle_a2\nneedle_a3\nneedle_a4\nneedle_a5\n";
    let hint = closest_match_hint("needle_a9", content).expect("hint expected");
    let separators = hint.matches("\n---\n").count();
    assert!(
        separators <= 2,
        "at most 3 snippets → at most 2 separators, got {separators}:\n{hint}"
    );
}

#[test]
fn test_closest_match_hint_cjk_near_cap_no_panic() {
    // CJK content large enough to exceed the byte cap: truncation must
    // land on a char boundary and never panic.
    let long_cjk_line = "中文内容行".repeat(200);
    let content = format!("{long_cjk_line}\n{long_cjk_line}\n{long_cjk_line}\n");
    let old_string = format!("{long_cjk_line}后缀");
    let hint = closest_match_hint(&old_string, &content).expect("hint expected");
    assert!(hint.len() <= 1_536);
    assert!(std::str::from_utf8(hint.as_bytes()).is_ok());
}

#[test]
fn test_closest_match_hint_empty_old_string_returns_none() {
    assert_eq!(closest_match_hint("", "some content"), None);
    assert_eq!(closest_match_hint("   \n  ", "some content"), None);
}
