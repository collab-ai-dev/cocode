//! Diff display widget — renders unified diff with color coding, line numbers,
//! word-level highlighting, and box-drawing structure.

use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use crate::diff::DiffLineViewRef;
use crate::diff::diff_line_view_refs;
use crate::diff::diff_line_view_window;
use crate::style::UiStyles;

// ── Box-drawing characters ──────────────────────────────────────────

const BOX_TOP_LEFT: &str = "╭";
const BOX_TOP_RIGHT: &str = "╮";
const BOX_BOTTOM_LEFT: &str = "╰";
const BOX_BOTTOM_RIGHT: &str = "╯";
const BOX_HORIZONTAL: &str = "─";
const BOX_VERTICAL: &str = "│";
const TAB_WIDTH: usize = 4;

// ── Word-level diff ─────────────────────────────────────────────────

/// Given two lines (one removed, one added), produce spans that highlight the
/// differing segments. Returns `(removed_spans, added_spans)`.
fn word_diff_spans(
    old_text: &str,
    new_text: &str,
    removed_style: Style,
    added_style: Style,
    emphasis_style: Style,
) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let old_chars: Vec<char> = old_text.chars().collect();
    let new_chars: Vec<char> = new_text.chars().collect();

    // Find common prefix length
    let prefix_len = old_chars
        .iter()
        .zip(new_chars.iter())
        .take_while(|(a, b)| a == b)
        .count();

    // Find common suffix length (after prefix)
    let old_remaining = &old_chars[prefix_len..];
    let new_remaining = &new_chars[prefix_len..];
    let suffix_len = old_remaining
        .iter()
        .rev()
        .zip(new_remaining.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    let old_diff_end = old_chars.len().saturating_sub(suffix_len);
    let new_diff_end = new_chars.len().saturating_sub(suffix_len);

    let old_prefix: String = old_chars[..prefix_len].iter().collect();
    let old_changed: String = old_chars[prefix_len..old_diff_end].iter().collect();
    let old_suffix: String = old_chars[old_diff_end..].iter().collect();

    let new_prefix: String = new_chars[..prefix_len].iter().collect();
    let new_changed: String = new_chars[prefix_len..new_diff_end].iter().collect();
    let new_suffix: String = new_chars[new_diff_end..].iter().collect();

    let removed_spans = vec![
        Span::styled(old_prefix, removed_style),
        Span::styled(
            old_changed,
            emphasis_style.fg(removed_style.fg.unwrap_or(ratatui::style::Color::Red)),
        ),
        Span::styled(old_suffix, removed_style),
    ];

    let added_spans = vec![
        Span::styled(new_prefix, added_style),
        Span::styled(
            new_changed,
            emphasis_style.fg(added_style.fg.unwrap_or(ratatui::style::Color::Green)),
        ),
        Span::styled(new_suffix, added_style),
    ];

    (removed_spans, added_spans)
}

// ── Line number formatting ──────────────────────────────────────────

/// Format a line number into a fixed-width string, or blanks if not applicable.
fn fmt_line_no(n: Option<i32>, width: usize) -> String {
    match n {
        Some(num) => format!("{num:>width$}"),
        None => " ".repeat(width),
    }
}

// ── Public API ──────────────────────────────────────────────────────

/// Render diff text as colored lines with line numbers and word-level
/// highlighting.
///
pub fn render_diff_lines(diff_text: &str, styles: UiStyles<'_>, width: u16) -> Vec<Line<'static>> {
    let rows = diff_line_view_refs(diff_text);
    render_rows(&rows, styles, width)
}

/// Render a bounded diff preview without first styling the full diff.
///
/// The parser still scans the whole diff to keep tail line numbers correct, but
/// only the retained head/tail rows are converted into ratatui lines. Long
/// source lines are hard-wrapped before the final screen-row cap is applied.
pub fn render_diff_preview_lines<F>(
    diff_text: &str,
    styles: UiStyles<'_>,
    width: u16,
    max_rows: usize,
    truncation_line: F,
) -> Vec<Line<'static>>
where
    F: Fn(usize) -> Line<'static>,
{
    if max_rows == 0 {
        return Vec::new();
    }
    let window = diff_line_view_window(diff_text, max_rows);
    let gutter_width =
        line_number_width_for_rows(window.head.iter().chain(window.tail.iter()).copied());
    let head = render_rows_with_gutter(&window.head, gutter_width, styles, width);
    let tail = render_rows_with_gutter(&window.tail, gutter_width, styles, width);
    combine_preview_lines(head, tail, window.omitted, max_rows, truncation_line)
}

fn render_rows(
    rows: &[DiffLineViewRef<'_>],
    styles: UiStyles<'_>,
    width: u16,
) -> Vec<Line<'static>> {
    let gutter_width = line_number_width(rows);
    render_rows_with_gutter(rows, gutter_width, styles, width)
}

fn render_rows_with_gutter(
    rows: &[DiffLineViewRef<'_>],
    gutter_width: usize,
    styles: UiStyles<'_>,
    width: u16,
) -> Vec<Line<'static>> {
    let mut result = Vec::new();

    for row in rows {
        result.extend(render_row(*row, gutter_width, styles, width));
    }

    result
}

fn render_row(
    row: DiffLineViewRef<'_>,
    gutter_width: usize,
    styles: UiStyles<'_>,
    width: u16,
) -> Vec<Line<'static>> {
    let removed_style = Style::new().fg(styles.diff_removed());
    let added_style = Style::new().fg(styles.diff_added());
    let emphasis = Style::new().reversed();

    match row {
        DiffLineViewRef::FileHeader { marker, path } => {
            vec![Line::from(vec![
                Span::raw(format!("  {marker} ")).fg(styles.dim()),
                Span::raw(path.to_string()).fg(styles.primary()).bold(),
            ])]
        }
        DiffLineViewRef::Hunk {
            old_start,
            new_start,
            label,
        } => {
            let label_part = if label.is_empty() {
                String::new()
            } else {
                format!(" {label}")
            };
            vec![Line::from(
                Span::raw(format!("  ╶╴ @@ -{old_start} +{new_start} @@{label_part}"))
                    .fg(styles.primary())
                    .dim(),
            )]
        }
        DiffLineViewRef::RawHunk { text } => {
            vec![Line::from(
                Span::raw(format!("  {text}")).fg(styles.primary()).dim(),
            )]
        }
        DiffLineViewRef::Context {
            old_line,
            new_line,
            content,
        } => {
            let old_no = fmt_line_no(Some(old_line), gutter_width);
            let new_no = fmt_line_no(Some(new_line), gutter_width);
            render_wrapped_content_line(
                vec![
                    Span::raw(format!("  {old_no} {new_no} "))
                        .fg(styles.dim())
                        .dim(),
                    Span::raw(format!("{BOX_VERTICAL} ")).fg(styles.border()),
                ],
                vec![Span::raw(content.to_string()).fg(styles.dim())],
                width,
                styles.dim(),
            )
        }
        DiffLineViewRef::Removed {
            old_line,
            content,
            compare_to,
        } => {
            let old_no = fmt_line_no(Some(old_line), gutter_width);
            let blank = fmt_line_no(None, gutter_width);
            let prefix = vec![
                Span::raw(format!("  {old_no} {blank} "))
                    .fg(styles.dim())
                    .dim(),
                Span::styled(
                    format!("{BOX_VERTICAL} "),
                    Style::new().fg(styles.diff_removed()),
                ),
            ];
            let content_spans = if let Some(compare_to) = compare_to {
                let (rm_spans, _) =
                    word_diff_spans(content, compare_to, removed_style, added_style, emphasis);
                let mut spans = vec![Span::styled("-", removed_style)];
                spans.extend(rm_spans);
                spans
            } else {
                vec![Span::styled(format!("-{content}"), removed_style)]
            };
            render_wrapped_content_line(prefix, content_spans, width, styles.dim())
        }
        DiffLineViewRef::Added {
            new_line,
            content,
            compare_to,
        } => {
            let blank = fmt_line_no(None, gutter_width);
            let new_no = fmt_line_no(Some(new_line), gutter_width);
            let prefix = vec![
                Span::raw(format!("  {blank} {new_no} "))
                    .fg(styles.dim())
                    .dim(),
                Span::styled(
                    format!("{BOX_VERTICAL} "),
                    Style::new().fg(styles.diff_added()),
                ),
            ];
            let content_spans = if let Some(compare_to) = compare_to {
                let (_, add_spans) =
                    word_diff_spans(compare_to, content, removed_style, added_style, emphasis);
                let mut spans = vec![Span::styled("+", added_style)];
                spans.extend(add_spans);
                spans
            } else {
                vec![Span::styled(format!("+{content}"), added_style)]
            };
            render_wrapped_content_line(prefix, content_spans, width, styles.dim())
        }
    }
}

fn render_wrapped_content_line(
    prefix: Vec<Span<'static>>,
    content: Vec<Span<'static>>,
    width: u16,
    continuation_color: ratatui::style::Color,
) -> Vec<Line<'static>> {
    let prefix_cols = spans_width(&prefix);
    let content_width = (width as usize).saturating_sub(prefix_cols).max(1);
    let chunks = wrap_styled_spans(&content, content_width);
    let continuation = Span::raw(" ".repeat(prefix_cols))
        .fg(continuation_color)
        .dim();
    let mut lines = Vec::with_capacity(chunks.len());

    for (index, chunk) in chunks.into_iter().enumerate() {
        let mut spans = if index == 0 {
            prefix.clone()
        } else {
            vec![continuation.clone()]
        };
        spans.extend(chunk);
        lines.push(Line::from(spans));
    }
    lines
}

fn combine_preview_lines<F>(
    head: Vec<Line<'static>>,
    tail: Vec<Line<'static>>,
    omitted: usize,
    max_rows: usize,
    truncation_line: F,
) -> Vec<Line<'static>>
where
    F: Fn(usize) -> Line<'static>,
{
    if max_rows == 0 {
        return Vec::new();
    }

    if omitted == 0 {
        let mut lines = head;
        lines.extend(tail);
        if lines.len() <= max_rows {
            return lines;
        }
        return cap_lines_middle(lines, max_rows, truncation_line);
    }

    if max_rows == 1 {
        return vec![truncation_line(omitted + head.len() + tail.len())];
    }

    let available = max_rows - 1;
    let mut head_take = head.len().min(available / 2);
    let mut tail_take = tail.len().min(available - head_take);
    let spare = available - head_take - tail_take;
    if spare > 0 {
        let extra_head = head.len().saturating_sub(head_take).min(spare);
        head_take += extra_head;
        let spare = spare - extra_head;
        tail_take += tail.len().saturating_sub(tail_take).min(spare);
    }

    let dropped = head.len().saturating_sub(head_take) + tail.len().saturating_sub(tail_take);
    let tail_skip = tail.len().saturating_sub(tail_take);
    let mut lines = Vec::with_capacity(head_take + 1 + tail_take);
    lines.extend(head.into_iter().take(head_take));
    lines.push(truncation_line(omitted + dropped));
    lines.extend(tail.into_iter().skip(tail_skip));
    lines
}

fn cap_lines_middle<F>(
    lines: Vec<Line<'static>>,
    max_rows: usize,
    truncation_line: F,
) -> Vec<Line<'static>>
where
    F: Fn(usize) -> Line<'static>,
{
    if max_rows == 0 || lines.is_empty() {
        return Vec::new();
    }
    if lines.len() <= max_rows {
        return lines;
    }
    if max_rows == 1 {
        return vec![truncation_line(lines.len())];
    }

    let available = max_rows - 1;
    let head_take = available / 2;
    let tail_take = available - head_take;
    let omitted = lines.len().saturating_sub(head_take + tail_take);
    let tail_start = lines.len().saturating_sub(tail_take);
    let mut capped = Vec::with_capacity(max_rows);
    capped.extend(lines.iter().take(head_take).cloned());
    capped.push(truncation_line(omitted));
    capped.extend(lines.iter().skip(tail_start).cloned());
    capped
}

fn wrap_styled_spans(spans: &[Span<'static>], max_cols: usize) -> Vec<Vec<Span<'static>>> {
    let mut result = Vec::new();
    let mut current_line = Vec::new();
    let mut col = 0usize;

    for span in spans {
        let style = span.style;
        let mut remaining = span.content.as_ref();

        while !remaining.is_empty() {
            let mut byte_end = 0usize;
            let mut chars_col = 0usize;

            for ch in remaining.chars() {
                let width = char_width(ch);
                if col + chars_col + width > max_cols {
                    break;
                }
                byte_end += ch.len_utf8();
                chars_col += width;
            }

            if byte_end == 0 {
                if !current_line.is_empty() {
                    result.push(std::mem::take(&mut current_line));
                    col = 0;
                    continue;
                }
                let Some(ch) = remaining.chars().next() else {
                    break;
                };
                let ch_len = ch.len_utf8();
                current_line.push(Span::styled(remaining[..ch_len].to_string(), style));
                col = char_width(ch).max(1);
                remaining = &remaining[ch_len..];
                continue;
            }

            let (chunk, rest) = remaining.split_at(byte_end);
            current_line.push(Span::styled(chunk.to_string(), style));
            col += chars_col;
            remaining = rest;

            if col >= max_cols {
                result.push(std::mem::take(&mut current_line));
                col = 0;
            }
        }
    }

    if !current_line.is_empty() || result.is_empty() {
        result.push(current_line);
    }

    result
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum()
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn char_width(ch: char) -> usize {
    ch.width().unwrap_or(if ch == '\t' { TAB_WIDTH } else { 0 })
}

fn line_number_width(rows: &[DiffLineViewRef<'_>]) -> usize {
    line_number_width_for_rows(rows.iter().copied())
}

fn line_number_width_for_rows<'a>(rows: impl Iterator<Item = DiffLineViewRef<'a>>) -> usize {
    let max = rows.filter_map(row_line_number).max().unwrap_or(0);
    max.to_string().len().max(1)
}

fn row_line_number(row: DiffLineViewRef<'_>) -> Option<i32> {
    match row {
        DiffLineViewRef::Context {
            old_line, new_line, ..
        } => Some(old_line.max(new_line)),
        DiffLineViewRef::Removed { old_line, .. } => Some(old_line),
        DiffLineViewRef::Added { new_line, .. } => Some(new_line),
        DiffLineViewRef::FileHeader { .. }
        | DiffLineViewRef::Hunk { .. }
        | DiffLineViewRef::RawHunk { .. } => None,
    }
}

/// Render a full-screen structured diff view with file path header, line
/// numbers, box-drawing border, and scroll support.
///
/// The `scroll` parameter controls which line is at the top of the viewport.
/// Negative values are clamped to 0.
pub fn render_structured_diff(
    path: &str,
    diff_text: &str,
    styles: UiStyles<'_>,
    width: u16,
    scroll: i32,
) -> Vec<Line<'static>> {
    let total_width = usize::from(width.max(2));
    let inner_width = total_width.saturating_sub(2).max(1);
    let horiz_border: String = BOX_HORIZONTAL.repeat(inner_width);

    let mut all_lines: Vec<Line<'static>> = Vec::new();

    // ── File header ─────────────────────────────────────────────
    all_lines.push(Line::from(vec![
        Span::raw(format!("{BOX_TOP_LEFT}{horiz_border}{BOX_TOP_RIGHT}")).fg(styles.border()),
    ]));
    let path_display = truncate_path(path, inner_width.saturating_sub(1));
    all_lines.push(Line::from(vec![
        Span::raw(format!("{BOX_VERTICAL} ")).fg(styles.border()),
        Span::raw(path_display).fg(styles.primary()).bold(),
    ]));
    all_lines.push(Line::from(vec![
        Span::raw(format!("{BOX_VERTICAL}{horiz_border}{BOX_VERTICAL}")).fg(styles.border()),
    ]));

    // ── Diff content ────────────────────────────────────────────
    let content_width = width.saturating_sub(1).max(1);
    let diff_lines = render_diff_lines(diff_text, styles, content_width);
    for line in diff_lines {
        // Re-wrap each line inside the box border
        let mut spans = vec![Span::raw(BOX_VERTICAL.to_string()).fg(styles.border())];
        spans.extend(line.spans);
        all_lines.push(Line::from(spans));
    }

    // ── Footer ──────────────────────────────────────────────────
    all_lines.push(Line::from(vec![
        Span::raw(format!("{BOX_BOTTOM_LEFT}{horiz_border}{BOX_BOTTOM_RIGHT}")).fg(styles.border()),
    ]));

    // ── Apply scroll offset ─────────────────────────────────────
    let offset = scroll.max(0) as usize;
    if offset >= all_lines.len() {
        return Vec::new();
    }
    all_lines.split_off(offset)
}

/// Truncate a path to fit within `max_len`, keeping the tail.
fn truncate_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        return path.to_string();
    }
    if max_len <= 3 {
        return "...".to_string();
    }
    let keep = max_len - 3;
    format!("...{}", &path[path.len() - keep..])
}

#[cfg(test)]
#[path = "diff_display.test.rs"]
mod tests;
