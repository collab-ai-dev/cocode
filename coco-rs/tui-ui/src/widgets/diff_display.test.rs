use super::*;
use crate::style::UiStyles;
use crate::theme::Theme;
use crate::theme::ThemeName;
use ratatui::style::Modifier;
use unicode_width::UnicodeWidthStr;

fn any_span_bg(lines: &[Line<'static>], bg: Color) -> bool {
    lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .any(|span| span.style.bg == Some(bg))
}

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
fn snapshot_diff_display_word_level_highlight() {
    // Visual golden: file headers, hunk marker, context, and a removed/added
    // pair with an intra-line word change. Locks gutter alignment + the
    // rendered text structure (styling is verified by the per-test assertions
    // above; this captures layout).
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let diff = "\
--- a/hello.rs
+++ b/hello.rs
@@ -1,2 +1,2 @@
 fn greet() {
-    println!(\"hello world\");
+    println!(\"hello, world!\");
 }";
    let lines = render_diff_lines(diff, styles, 60, DiffHighlight::default());
    insta::assert_snapshot!("diff_display_word_highlight", text_of(&lines));
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
    let lines = render_diff_lines(diff, styles, 80, DiffHighlight::default());
    // File headers(2) + hunk(1) + context(1) + paired old/new(2) + added(1) = 7
    assert_eq!(lines.len(), 7);
}

#[test]
fn test_render_diff_lines_empty() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let lines = render_diff_lines("", styles, 80, DiffHighlight::default());
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
    let lines = render_diff_lines(&long, styles, 20, DiffHighlight::default());

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
    let lines =
        render_diff_preview_lines(&diff, styles, 80, 5, DiffHighlight::default(), |omitted| {
            Line::from(Span::raw(format!("… +{omitted} lines")))
        });
    let text = text_of(&lines);

    assert_eq!(lines.len(), 5);
    assert!(text.contains("line 0"), "{text}");
    assert!(text.contains("line 49"), "{text}");
    assert!(text.contains("… +"), "{text}");
}

// ── D1: background tint + syntax injection + separators ──────────────

#[test]
fn added_and_removed_rows_carry_the_theme_background_tint() {
    let theme = Theme::default(); // dark: both diff bg tints are Some
    let styles = UiStyles::new(&theme);
    let added_bg = theme.diff_added_bg.expect("dark theme tints added");
    let removed_bg = theme.diff_removed_bg.expect("dark theme tints removed");
    let diff = "@@ -1,1 +1,1 @@\n-old\n+new";
    let lines = render_diff_lines(diff, styles, 40, DiffHighlight::default());
    assert!(any_span_bg(&lines, added_bg), "added row must be tinted");
    assert!(
        any_span_bg(&lines, removed_bg),
        "removed row must be tinted"
    );
}

#[test]
fn ansi_theme_leaves_diff_rows_untinted() {
    // ANSI themes set the bg tints to None — the fg marker carries the diff.
    let theme = Theme::from_name(ThemeName::DarkAnsi);
    let styles = UiStyles::new(&theme);
    let diff = "@@ -1,1 +1,1 @@\n-old\n+new";
    let lines = render_diff_lines(diff, styles, 40, DiffHighlight::default());
    assert!(
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .all(|span| span.style.bg.is_none()),
        "ANSI theme must not emit any background"
    );
}

#[test]
fn injected_syntax_spans_replace_plain_content() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    // A single added line at new_line == 1 pulls token spans from `new[0]`.
    let new_tokens = vec![vec![Span::styled("TOKEN", Style::new().fg(Color::Magenta))]];
    let hl = DiffHighlight {
        old: &[],
        new: &new_tokens,
    };
    let lines = render_diff_lines("+plain", styles, 40, hl);
    let text = text_of(&lines);
    assert!(text.contains("TOKEN"), "token spans should render: {text}");
    assert!(!text.contains("plain"), "raw content replaced: {text}");
    // The token keeps its syntax fg and gains the diff tint behind it.
    let tok = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "TOKEN")
        .expect("token span present");
    assert_eq!(tok.style.fg, Some(Color::Magenta));
    assert_eq!(tok.style.bg, theme.diff_added_bg);
}

#[test]
fn syntax_highlighting_and_word_diff_emphasis_are_composed() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let old_tokens = vec![vec![
        Span::styled("let value = ", Style::new().fg(Color::Blue)),
        Span::styled("old", Style::new().fg(Color::Magenta)),
        Span::styled(";", Style::new().fg(Color::Yellow)),
    ]];
    let new_tokens = vec![vec![
        Span::styled("let value = ", Style::new().fg(Color::Blue)),
        Span::styled("new", Style::new().fg(Color::Cyan)),
        Span::styled(";", Style::new().fg(Color::Yellow)),
    ]];
    let hl = DiffHighlight {
        old: &old_tokens,
        new: &new_tokens,
    };
    let lines = render_diff_lines(
        "@@ -1,1 +1,1 @@\n-let value = old;\n+let value = new;",
        styles,
        80,
        hl,
    );

    let old = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "old")
        .expect("old token");
    let new = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "new")
        .expect("new token");
    assert_eq!(old.style.fg, Some(Color::Magenta));
    assert_eq!(new.style.fg, Some(Color::Cyan));
    assert!(old.style.add_modifier.contains(Modifier::REVERSED));
    assert!(new.style.add_modifier.contains(Modifier::REVERSED));
    assert_eq!(old.style.bg, theme.diff_removed_bg);
    assert_eq!(new.style.bg, theme.diff_added_bg);
    insta::assert_debug_snapshot!("syntax_and_word_diff_styles", lines);
}

#[test]
fn hunk_boundary_shows_unchanged_line_count() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    // Two hunks: the first ends at new-line 1, the second starts at new-line 5,
    // so three unchanged lines were skipped between them.
    let diff = "@@ -1,1 +1,1 @@\n alpha\n@@ -5,1 +5,1 @@\n beta";
    let lines = render_diff_lines(diff, styles, 40, DiffHighlight::default());
    let text = text_of(&lines);
    assert!(
        text.contains("⋯ 3 unchanged lines"),
        "expected a skipped-line separator: {text}"
    );
    // The first hunk has no known predecessor, so no separator precedes it.
    assert_eq!(text.matches("unchanged line").count(), 1, "{text}");
}

#[test]
fn single_skipped_line_uses_singular_wording() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let diff = "@@ -1,1 +1,1 @@\n alpha\n@@ -3,1 +3,1 @@\n gamma";
    let lines = render_diff_lines(diff, styles, 40, DiffHighlight::default());
    let text = text_of(&lines);
    assert!(text.contains("⋯ 1 unchanged line"), "singular: {text}");
    assert!(
        !text.contains("1 unchanged lines"),
        "no plural for 1: {text}"
    );
}

#[test]
fn truncate_path_handles_multibyte_tail_without_panic() {
    // A multi-byte path longer than max_len must not panic slicing mid-char,
    // and the result must stay within the byte budget.
    let path = "src/文件目录/名前ファイル.rs";
    let out = truncate_path(path, 12);
    assert!(out.starts_with("..."));
    assert!(
        out.len() <= 12,
        "over budget: {out:?} ({} bytes)",
        out.len()
    );
}
