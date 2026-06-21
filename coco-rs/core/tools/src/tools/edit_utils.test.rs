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
