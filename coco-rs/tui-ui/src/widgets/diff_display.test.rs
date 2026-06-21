use super::*;
use crate::style::UiStyles;
use crate::theme::Theme;
use unicode_width::UnicodeWidthStr;

fn text_of(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_width(line: &Line<'static>) -> usize {
    line.spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

#[test]
fn test_render_diff_lines_basic() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +1,4 @@
 context
-old line
+new line
+added";
    let lines = render_diff_lines(diff, styles, 80);
    // File headers(2) + hunk(1) + context(1) + paired old/new(2) + added(1) = 7
    assert_eq!(lines.len(), 7);
}

#[test]
fn test_render_diff_lines_empty() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let lines = render_diff_lines("", styles, 80);
    assert!(lines.is_empty());
}

#[test]
fn test_render_structured_diff_scroll_past_end() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let diff = "+one\n+two";
    let lines = render_structured_diff("test.rs", diff, styles, 80, 9999);
    assert!(lines.is_empty());
}

#[test]
fn test_render_structured_diff_negative_scroll() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let diff = "+one";
    let all = render_structured_diff("test.rs", diff, styles, 80, 0);
    let neg = render_structured_diff("test.rs", diff, styles, 80, -5);
    assert_eq!(all.len(), neg.len());
}

#[test]
fn test_render_structured_diff_rows_stay_within_requested_width() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let diff = format!("+{}", "abcdef".repeat(20));
    let width = 32;
    let lines = render_structured_diff("test.rs", &diff, styles, width, 0);
    let text = text_of(&lines);

    assert!(!lines.is_empty(), "structured diff should render");
    assert!(
        lines
            .iter()
            .all(|line| line_width(line) <= usize::from(width)),
        "all rows must fit width {width}:\n{text}"
    );
}

#[test]
fn test_truncate_path_short() {
    assert_eq!(truncate_path("foo.rs", 20), "foo.rs");
}

#[test]
fn test_truncate_path_long() {
    let long = "a/very/long/path/to/some/deeply/nested/file.rs";
    let result = truncate_path(long, 20);
    assert!(result.starts_with("..."));
    assert!(result.len() <= 20);
}

#[test]
fn test_truncate_path_tiny_max() {
    assert_eq!(truncate_path("abcdefgh", 3), "...");
}

#[test]
fn test_fmt_line_no_some() {
    assert_eq!(fmt_line_no(Some(42), 4), "  42");
}

#[test]
fn test_fmt_line_no_none() {
    assert_eq!(fmt_line_no(None, 4), "    ");
}

#[test]
fn test_render_diff_lines_wraps_long_signed_lines() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let long = format!("+{}", "abcdef".repeat(8));
    let lines = render_diff_lines(&long, styles, 20);

    assert!(lines.len() > 1, "long diff line should wrap");
    assert!(
        text_of(&lines)
            .lines()
            .skip(1)
            .all(|line| !line.contains('+')),
        "continuation rows should not repeat the sign: {}",
        text_of(&lines)
    );
}

#[test]
fn test_render_diff_preview_lines_caps_without_full_render() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let diff = (0..50)
        .map(|i| format!("+line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = render_diff_preview_lines(&diff, styles, 80, 5, |omitted| {
        Line::from(Span::raw(format!("… +{omitted} lines")))
    });
    let text = text_of(&lines);

    assert_eq!(lines.len(), 5);
    assert!(text.contains("line 0"), "{text}");
    assert!(text.contains("line 49"), "{text}");
    assert!(text.contains("… +"), "{text}");
}
