use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::theme::Theme;
use pretty_assertions::assert_eq;
use ratatui::style::Style;

use super::*;

/// Flatten a line's spans to plain text (drops styling).
fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn render(text: &str) -> Vec<Line<'static>> {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    render_markdown(
        text,
        MarkdownOptions::new(styles, 80, SyntaxHighlighting::Enabled),
        None,
    )
}

fn render_text(text: &str) -> Vec<String> {
    render(text).iter().map(line_text).collect()
}

fn render_with_width(text: &str, width: u16) -> Vec<Line<'static>> {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    render_markdown(
        text,
        MarkdownOptions::new(styles, width, SyntaxHighlighting::Enabled),
        None,
    )
}

fn strip_code_gutter(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix("│ ")
}

fn stable_boundaries(source: &str) -> Vec<usize> {
    let mut boundaries = Vec::new();
    for end in source
        .char_indices()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .filter(|end| source[..*end].ends_with('\n'))
    {
        let stable_end = stable_prefix_end(&source[..end]);
        if stable_end > 0 && boundaries.last() != Some(&stable_end) {
            boundaries.push(stable_end);
        }
    }
    boundaries
}

fn progressive_sources() -> &'static [&'static str] {
    &[
        "alpha\n\nbeta\n\n",
        "# Heading\nparagraph\n\n",
        "Title\n---\n\nbody\n\n",
        "- one\n- two\n\nnext\n\n",
        "- alpha\n\n- beta\n\n- gamma\n\nnext\n\n",
        "intro\n\n1. first\n\n2. second\n\n---\n\nafter\n\n",
        "- outer\n  - inner\n  continuation\n\n",
        "> quoted\n> continuation\n\noutside\n\n",
        "lazy\ncontinuation\n\nnext\n\n",
        "- [ ] todo\n\nnext\n\n",
        "Use [inline](https://example.com)\n\nnext\n\n",
        "| a | b |\n| - | - |\n| 1 | 2 |\n\nnext\n\n",
        "```rust\nfn main() {}\n```\nafter\n\n",
        "[label]\n\n[label]: https://example.com\n\n",
        "<div>\nraw html\n</div>\n\nafter\n\n",
        "```mermaid\nflowchart LR\n  A[Start] --> B[Finish]\n```\n\nafter\n\n",
    ]
}

#[test]
fn paragraph_is_indented_two_columns() {
    let lines = render_text("hello world");
    assert_eq!(lines, vec!["  hello world".to_string()]);
}

#[test]
fn soft_breaks_preserve_authored_lines() {
    let lines = render_text("first line\nsecond line");
    assert_eq!(
        lines,
        vec!["  first line".to_string(), "  second line".to_string()]
    );
}

#[test]
fn inline_code_uses_code_inline_token_matching_ts_permission() {
    // Inline `code` spans paint with the `code_inline` token. TS renders
    // codespan via `color('permission')`, so the default `code_inline` mirrors
    // the periwinkle `accent` (= permission) rather than a syntax-highlight
    // color — this is why inline code reads "light blue", not pink. The token
    // stays a distinct field so custom (`theme.json`) themes can override it.
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let lines = render_markdown(
        "use `xyzzy` here",
        MarkdownOptions::new(styles, 80, SyntaxHighlighting::Enabled),
        None,
    );
    let code = lines[0]
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "xyzzy")
        .expect("an inline-code span");
    assert_eq!(code.style.fg, Some(theme.code_inline));
    assert_eq!(
        theme.code_inline, theme.accent,
        "default inline code mirrors TS `color('permission')`"
    );
}

#[test]
fn list_marker_uses_body_text_color_not_brand() {
    // TS renders the list marker (`-` / `N.`) with no color wrapper, so it
    // inherits the body text color. coco mirrors that: the bullet must NOT use
    // the brand `primary` (claude orange), which downsampled to a harsh yellow.
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let lines = render_markdown(
        "- item",
        MarkdownOptions::new(styles, 80, SyntaxHighlighting::Enabled),
        None,
    );
    let bullet = lines[0]
        .spans
        .iter()
        .find(|s| s.content.as_ref().contains('•'))
        .expect("a bullet span");
    assert_eq!(bullet.style.fg, Some(theme.text));
    assert_ne!(theme.text, theme.primary, "bullet must not use brand color");
}

#[test]
fn heading_renders_without_hash_prefix() {
    let lines = render_text("# Title");
    assert_eq!(lines, vec!["  Title".to_string()]);
}

#[test]
fn unordered_list_uses_bullet_marker() {
    let lines = render_text("- one\n- two");
    assert_eq!(lines, vec!["  • one".to_string(), "  • two".to_string()]);
}

#[test]
fn ordered_list_numbers_items() {
    let lines = render_text("1. first\n2. second");
    assert_eq!(
        lines,
        vec!["  1. first".to_string(), "  2. second".to_string()]
    );
}

#[test]
fn fenced_code_block_has_border_and_body() {
    let lines = render_text("```rust\nlet x = 1;\n```");
    assert!(
        lines
            .first()
            .is_some_and(|l| l.trim_start().starts_with('┌'))
    );
    assert!(lines.iter().any(|l| l.contains("let x = 1;")));
    assert!(
        lines
            .last()
            .is_some_and(|l| l.trim_start().starts_with('└'))
    );
}

#[test]
fn long_plain_fenced_code_wraps_within_narrow_width() {
    let width = 24;
    let lines: Vec<String> = render_with_width(
        "```\nabcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz\n```",
        width,
    )
    .iter()
    .map(line_text)
    .collect();
    for line in lines.iter().filter(|l| !l.trim().is_empty()) {
        assert!(
            coco_tui_ui::truncate::display_width(line) <= width as usize,
            "line overflows width {width}: {} ({line:?})",
            coco_tui_ui::truncate::display_width(line)
        );
    }
    let body_lines: Vec<&String> = lines
        .iter()
        .filter(|l| l.trim_start().starts_with("│ "))
        .collect();
    assert!(
        body_lines.len() > 1,
        "expected wrapped body lines: {lines:?}"
    );
    assert!(
        body_lines.iter().all(|l| strip_code_gutter(l).is_some()),
        "body continuation line missing gutter: {lines:?}"
    );
}

#[test]
fn long_code_fence_inside_list_item_wraps_within_width() {
    let width = 24;
    let lines: Vec<String> = render_with_width(
        "- intro\n  ```\n  abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz\n  ```",
        width,
    )
    .iter()
    .map(line_text)
    .collect();
    for line in lines.iter().filter(|l| !l.trim().is_empty()) {
        assert!(
            coco_tui_ui::truncate::display_width(line) <= width as usize,
            "list code-fence line overflows width {width}: {} ({line:?})",
            coco_tui_ui::truncate::display_width(line)
        );
    }
}

#[test]
fn wide_code_glyph_at_one_column_body_budget_stays_within_width() {
    let width = 5;
    let lines: Vec<String> = render_with_width("```\n中文\n```", width)
        .iter()
        .map(line_text)
        .collect();
    for line in lines.iter().filter(|l| !l.trim().is_empty()) {
        assert!(
            coco_tui_ui::truncate::display_width(line) <= width as usize,
            "wide code glyph line overflows width {width}: {} ({line:?})",
            coco_tui_ui::truncate::display_width(line)
        );
    }
}

#[test]
fn long_highlighted_rust_code_wraps_without_dropping_styles() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let code = "fn main() { let value: i32 = 12345; println!(\"abcdefghijklmnopqrstuvwxyz\"); }";
    let lines = render_markdown(
        &format!("```rust\n{code}\n```"),
        MarkdownOptions::new(styles, 32, SyntaxHighlighting::Enabled),
        None,
    );
    let body_lines: Vec<&Line<'_>> = lines
        .iter()
        .filter(|line| line_text(line).trim_start().starts_with("│ "))
        .collect();
    assert!(body_lines.len() > 1, "expected wrapped rust line");

    let rendered_code: String = body_lines
        .iter()
        .filter_map(|line| strip_code_gutter(&line_text(line)).map(ToString::to_string))
        .collect();
    assert_eq!(rendered_code, code);

    let kept_highlight_style = body_lines.iter().any(|line| {
        line.spans
            .iter()
            .filter(|span| span.content.as_ref() != "│ " && !span.content.trim().is_empty())
            .any(|span| span.style.fg.is_some_and(|fg| fg != theme.text))
    });
    assert!(kept_highlight_style, "highlight styles were dropped");
}

#[test]
fn table_renders_box_grid() {
    let md = "| a | b |\n| - | - |\n| 1 | 2 |";
    let lines = render_text(md);
    assert!(lines.iter().any(|l| l.trim_start().starts_with('┌')));
    assert!(lines.iter().any(|l| l.contains('a') && l.contains('b')));
    assert!(lines.iter().any(|l| l.contains('1') && l.contains('2')));
    assert!(lines.iter().any(|l| l.trim_start().starts_with('└')));
}

#[test]
fn gfm_alert_emits_labeled_header() {
    let lines = render_text("> [!WARNING]\n> be careful");
    assert!(lines.iter().any(|l| l.contains("WARNING")));
    assert!(lines.iter().any(|l| l.contains("be careful")));
}

#[test]
fn lead_marker_lands_on_first_line() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let marker = LeadMarker::new("⏺", styles.assistant_message());
    let opts = MarkdownOptions::new(styles, 80, SyntaxHighlighting::Enabled);
    let lines = render_markdown("hello\nworld", opts, Some(&marker));
    assert_eq!(line_text(&lines[0]), "⏺ hello");
    assert_eq!(line_text(&lines[1]), "  world");
}

#[test]
fn empty_text_with_marker_emits_marker_line() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let marker = LeadMarker::new("⏺", styles.assistant_message());
    let opts = MarkdownOptions::new(styles, 80, SyntaxHighlighting::Enabled);
    let lines = render_markdown("", opts, Some(&marker));
    assert_eq!(lines.len(), 1);
    assert_eq!(line_text(&lines[0]), "⏺");
}

#[test]
fn empty_text_without_marker_is_empty() {
    let _ = Style::default();
    assert!(render("").is_empty());
}

#[test]
fn stable_prefix_holds_back_unterminated_line() {
    assert_eq!(stable_prefix_end("one\ntwo"), 0);
}

#[test]
fn stable_prefix_uses_blank_line_boundary() {
    assert_eq!(stable_prefix_end("one\n\ntwo"), "one\n\n".len());
}

#[test]
fn stable_prefix_holds_back_setext_heading_candidate() {
    assert_eq!(stable_prefix_end("Title\n---"), 0);
    assert_eq!(stable_prefix_end("Title\n---\nbody"), 0);
    assert_eq!(
        stable_prefix_end("Title\n---\n\nbody"),
        "Title\n---\n\n".len()
    );
}

#[test]
fn stable_prefix_holds_back_reference_link_candidate() {
    assert_eq!(stable_prefix_end("[label]\n\nbody"), 0);
}

#[test]
fn stable_prefix_allows_task_list_checkbox() {
    assert_eq!(
        stable_prefix_end("- [ ] todo\n\nbody"),
        "- [ ] todo\n\n".len()
    );
    assert_eq!(
        stable_prefix_end("- [x] done\n\nbody"),
        "- [x] done\n\n".len()
    );
}

#[test]
fn stable_prefix_allows_inline_link() {
    let prefix = "Use [inline](https://example.com)\n\n";
    assert_eq!(stable_prefix_end(&format!("{prefix}body")), prefix.len());
}

#[test]
fn stable_prefix_holds_back_unresolved_full_reference_link() {
    assert_eq!(stable_prefix_end("[label][missing]\n\nbody"), 0);
}

#[test]
fn stable_prefix_allows_resolved_reference_link_after_definition() {
    let prefix = "[label][target]\n\n[target]: https://example.com\n\n";
    assert_eq!(stable_prefix_end(&format!("{prefix}body")), prefix.len());
}

#[test]
fn stable_prefix_allows_atx_heading_without_blank_line() {
    assert_eq!(stable_prefix_end("# Heading\nbody"), "# Heading\n".len());
}

#[test]
fn stable_prefix_tracks_fence_marker_length() {
    let source = "````\n```rust\nlet value = 1;\n```\n";
    assert_eq!(stable_prefix_end(source), 0);
    assert_eq!(
        stable_prefix_end("````\n```rust\nlet value = 1;\n```\n````\n\nbody"),
        "````\n```rust\nlet value = 1;\n```\n````\n\n".len()
    );
}

#[test]
fn stable_prefix_allows_closed_code_fence_without_blank_line() {
    let source = "```rust\nlet values = [1, 2, 3];\n```\nbody";
    assert_eq!(
        stable_prefix_end(source),
        "```rust\nlet values = [1, 2, 3];\n```\n".len()
    );
}

#[test]
fn stable_prefix_rejects_invalid_closing_code_fence() {
    let source = "```rust\nlet value = 1;\n``` not closed\nbody\n";
    assert_eq!(stable_prefix_end(source), 0);
}

#[test]
fn stable_prefix_does_not_close_fence_on_info_string() {
    let source = "```\n```rust\nlet value = 1;\n```\nbody";
    assert_eq!(
        stable_prefix_end(source),
        "```\n```rust\nlet value = 1;\n```\n".len()
    );
}

#[test]
fn stable_prefix_holds_back_markdown_table_region() {
    let source = "intro\n| a | b |\n| - | - |\n| 1 | 2 |\n";
    assert_eq!(stable_prefix_end(source), 0);
}

#[test]
fn stable_prefix_holds_back_open_code_fence() {
    let source = "intro\n```rust\nfn main() {}\n";
    assert_eq!(stable_prefix_end(source), 0);
}

#[test]
fn stable_prefix_holds_back_trailing_open_list() {
    // A later sibling item would flip the list tight→loose and rewrite the
    // already-rendered items, so a still-growing trailing list never commits.
    assert_eq!(stable_prefix_end("intro\n\n- alpha\n\n"), "intro\n\n".len());
    assert_eq!(
        stable_prefix_end("intro\n\n- alpha\n\n- beta\n\n"),
        "intro\n\n".len()
    );
    assert_eq!(
        stable_prefix_end("intro\n\n1. first\n\n"),
        "intro\n\n".len()
    );
}

#[test]
fn stable_prefix_releases_list_interrupted_by_paragraph() {
    // An unindented paragraph after a blank line ends the list — the whole
    // region becomes committable at the next boundary.
    let source = "- alpha\n\n- beta\n\nclosing paragraph\n\n";
    assert_eq!(stable_prefix_end(source), source.len());
    // The unterminated tail can prove the interruption too ('c' can never
    // grow into a list marker)…
    assert_eq!(stable_prefix_end("- alpha\n\nclosing"), "- alpha\n\n".len());
    // …but an ambiguous starter could still become a sibling item.
    assert_eq!(stable_prefix_end("- alpha\n\n- "), 0);
    assert_eq!(stable_prefix_end("- alpha\n\n1"), 0);
}

#[test]
fn stable_prefix_releases_list_interrupted_by_heading_or_break() {
    assert_eq!(
        stable_prefix_end("- alpha\n\n---\n\nbody"),
        "- alpha\n\n---\n\n".len()
    );
    assert_eq!(
        stable_prefix_end("- alpha\n# heading\nbody\n\n"),
        "- alpha\n# heading\nbody\n\n".len()
    );
}

#[test]
fn stable_prefix_keeps_lazy_continuation_in_list_hold() {
    // "lazy" continues the item's paragraph (no blank between) — the list is
    // still open, so nothing past the intro commits.
    assert_eq!(
        stable_prefix_end("intro\n\n- alpha\nlazy\n\n"),
        "intro\n\n".len()
    );
}

#[test]
fn stable_prefix_render_matches_finalized_line_prefix() {
    for source in progressive_sources() {
        let full = render(source);
        for end in stable_boundaries(source) {
            let prefix = render(&source[..end]);
            assert!(
                full.starts_with(&prefix),
                "stable render diverged at byte {end} for source:\n{source}\nprefix:\n{:#?}\nfull:\n{:#?}",
                render_text(&source[..end]),
                render_text(source),
            );
        }
    }
}

#[test]
fn list_item_text_survives_following_block() {
    // Regression: a list item's text immediately followed by a block (code
    // fence) must not be dropped (was overwritten by the raw block line).
    let joined = render_text("- step one\n  ```\n  code\n  ```\n").join("\n");
    assert!(joined.contains("step one"), "item text dropped:\n{joined}");
    assert!(joined.contains("code"), "code body dropped:\n{joined}");
}

#[test]
fn nested_list_preserves_parent_text_and_marker() {
    // Regression: parent item text + marker must render on their own line
    // before descending, not merge with the first child.
    let lines = render_text("- outer\n  - inner1\n  - inner2\n");
    assert!(
        lines.iter().any(|l| l.trim() == "• outer"),
        "parent merged/lost:\n{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("inner1")));
    assert!(lines.iter().any(|l| l.contains("inner2")));
}

#[test]
fn task_list_uses_checkbox_not_redundant_bullet() {
    let lines = render_text("- [ ] todo\n- [x] done\n");
    assert_eq!(lines, vec!["  ☐ todo".to_string(), "  ☑ done".to_string()]);
}

#[test]
fn list_continuation_hangs_under_item_text() {
    let lines = render_text("- line1\n  line2\n");
    assert_eq!(
        lines,
        vec!["  • line1".to_string(), "    line2".to_string()]
    );
}

#[test]
fn multiline_html_block_keeps_line_structure() {
    let joined = render_text("before\n\n<div>\nraw html\n</div>\n\nafter").join("\n");
    assert!(joined.contains("<div>"), "{joined}");
    assert!(joined.contains("raw html"), "{joined}");
    assert!(joined.contains("</div>"), "{joined}");
}

#[cfg(feature = "mermaid")]
#[test]
fn mermaid_fence_renders_as_cells() {
    let lines = render("```mermaid\nflowchart LR\n  A[Start] --> B[Finish]\n```");
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        joined.contains("Start") && joined.contains("Finish"),
        "node labels missing:\n{joined}"
    );
    assert!(
        joined
            .chars()
            .any(|c| matches!(c, '╭' | '╮' | '╰' | '╯' | '│' | '─')),
        "expected box-drawing:\n{joined}"
    );
    // Rendered as a diagram, not the verbatim `flowchart LR` source line.
    assert!(
        !joined.contains("flowchart LR"),
        "fell back to verbatim:\n{joined}"
    );
}

#[test]
fn table_with_wide_cell_keeps_uniform_row_width() {
    // F3: a CJK cell wider than the column cap must not leave its row a column
    // short — pad_cell re-pads to the exact column width after truncation.
    let md = "| a | b |\n| - | - |\n| 你好世界你好世界 | y |";
    let widths: Vec<usize> = render_text(md)
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| coco_tui_ui::truncate::display_width(l))
        .collect();
    assert!(widths.len() >= 4, "expected a full table grid: {widths:?}");
    assert!(
        widths.windows(2).all(|w| w[0] == w[1]),
        "table rows have differing display widths: {widths:?}"
    );
}

#[test]
fn sibling_ordered_lists_restart_numbering() {
    let lines = render_text("1. a\n2. b\n\ntext\n\n1. c\n2. d");
    assert!(lines.iter().any(|l| l.contains("1. a")));
    assert!(
        lines.iter().any(|l| l.contains("1. c")),
        "second ordered list did not restart at 1: {lines:?}"
    );
}

#[test]
fn rule_in_blockquote_fits_width() {
    // F4: the rule budget subtracts the full left margin (indent + quote gutter),
    // so it does not overflow the 80-col width inside a blockquote.
    let lines = render_text("> intro\n>\n> ---\n");
    let rule = lines
        .iter()
        .find(|l| l.contains('─'))
        .expect("rule rendered");
    assert!(
        coco_tui_ui::truncate::display_width(rule) <= 80,
        "blockquote rule overflows width: {} ({rule:?})",
        coco_tui_ui::truncate::display_width(rule)
    );
}

#[test]
fn code_fence_in_blockquote_fits_width() {
    // F4: fence border + bg-padded body budget from the full left margin.
    let lines = render_text("> ```\n> code here\n> ```\n");
    for l in lines.iter().filter(|l| !l.trim().is_empty()) {
        assert!(
            coco_tui_ui::truncate::display_width(l) <= 80,
            "blockquote code-fence line overflows width: {} ({l:?})",
            coco_tui_ui::truncate::display_width(l)
        );
    }
}

#[test]
fn long_code_fence_in_blockquote_wraps_within_configured_width() {
    let width = 28;
    let lines: Vec<String> = render_with_width(
        "> ```\n> abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz\n> ```\n",
        width,
    )
    .iter()
    .map(line_text)
    .collect();
    for line in lines.iter().filter(|l| !l.trim().is_empty()) {
        assert!(
            coco_tui_ui::truncate::display_width(line) <= width as usize,
            "blockquote code-fence line overflows width {width}: {} ({line:?})",
            coco_tui_ui::truncate::display_width(line)
        );
    }
}

#[test]
fn long_code_fence_language_label_is_truncated_to_width() {
    let width = 18;
    let lines: Vec<String> =
        render_with_width("```averyveryverylonglanguagelabel\ncode\n```", width)
            .iter()
            .map(line_text)
            .collect();
    let header = lines.first().expect("code fence header");
    assert!(header.trim_start().starts_with("┌─"));
    assert!(
        coco_tui_ui::truncate::display_width(header) <= width as usize,
        "header overflows width {width}: {} ({header:?})",
        coco_tui_ui::truncate::display_width(header)
    );
}

#[cfg(feature = "mermaid")]
#[test]
fn streaming_suppresses_mermaid_diagram() {
    // The same diagram-capable fence must NOT be laid out while streaming —
    // it stays verbatim so the block doesn't reflow on every delta.
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let src = "```mermaid\nflowchart LR\n  A[Start] --> B[Finish]\n```";
    let lines = render_markdown(
        src,
        MarkdownOptions::new(styles, 80, SyntaxHighlighting::Enabled).streaming(),
        None,
    );
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        joined.contains("flowchart LR"),
        "streaming should keep the verbatim source, got:\n{joined}"
    );
}

// ── Inline link destination rendering (G1 regression) ──
//
// Mirrors codex tui/src/markdown_render_tests.rs link coverage. coco has no
// OSC 8 plumbing, so links degrade to the terminal-fallback form (the URL is
// shown inline) rather than being silently dropped.

#[test]
fn inline_link_preserves_destination_url() {
    let text = render_text("[click here](https://example.com/path)").join("\n");
    assert!(text.contains("click here"), "missing link text: {text:?}");
    assert!(
        text.contains("https://example.com/path"),
        "link destination dropped: {text:?}"
    );
}

#[test]
fn autolink_does_not_duplicate_destination() {
    // An autolink's display text already equals its destination; rendering must
    // not produce `url (url)`.
    let text = render_text("<https://example.com>").join("\n");
    assert_eq!(
        text.matches("https://example.com").count(),
        1,
        "autolink destination duplicated: {text:?}"
    );
    assert!(!text.contains("(https://"), "spurious suffix: {text:?}");
}

#[test]
fn bare_url_keeps_single_destination() {
    // Robust whether or not pulldown GFM-autolinks the bare URL: either way the
    // URL appears verbatim once and is never duplicated with a `(url)` suffix.
    let text = render_text("see https://example.com now").join("\n");
    assert!(
        text.contains("https://example.com"),
        "bare URL dropped: {text:?}"
    );
    assert!(!text.contains("(https://"), "bare URL duplicated: {text:?}");
}

#[test]
fn mailto_link_shows_bare_address_without_scheme() {
    let text = render_text("[Email me](mailto:user@example.com)").join("\n");
    assert!(text.contains("Email me"), "missing link text: {text:?}");
    assert!(
        text.contains("user@example.com"),
        "email address dropped: {text:?}"
    );
    assert!(
        !text.contains("mailto:"),
        "mailto scheme leaked into output: {text:?}"
    );
}

#[test]
fn inline_link_destination_uses_hyperlink_style() {
    let theme = Theme::default();
    let expected = UiStyles::new(&theme).hyperlink();
    let lines = render_markdown(
        "[click](https://example.com)",
        MarkdownOptions::new(UiStyles::new(&theme), 80, SyntaxHighlighting::Enabled),
        None,
    );
    let dest_span = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains("https://example.com"))
        .expect("destination span present");
    assert_eq!(dest_span.style.fg, Some(expected));
}
