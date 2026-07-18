//! Diff display widget — renders unified diff with color coding, line numbers,
//! word-level highlighting, and box-drawing structure.

use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use std::ops::Range;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use crate::diff::DiffLineViewRef;
use crate::diff::diff_line_view_refs;
use crate::diff::diff_line_view_window;
use crate::style::UiStyles;

// ── Syntax-highlight injection (domain-free seam) ───────────────────

/// Pre-highlighted diff content handed in by the shell, which owns syntect.
///
/// The shell highlights the old- and new-side content as blocks and passes the
/// per-line token spans here, indexed by 1-based source line number. The widget
/// looks each row up by its `old_line` / `new_line` and layers the diff
/// background tint over the tokens. The default (both slices empty) means "no
/// syntax highlighting" — the widget renders plain diff-colored text, so callers
/// that have no source context just pass `DiffHighlight::default()`.
#[derive(Clone, Copy, Default)]
pub struct DiffHighlight<'a> {
    /// Token spans for the old (removed / context) side, per source line.
    pub old: &'a [Vec<Span<'static>>],
    /// Token spans for the new (added / context) side, per source line.
    pub new: &'a [Vec<Span<'static>>],
}

impl<'a> DiffHighlight<'a> {
    fn line(rows: &'a [Vec<Span<'static>>], n: i32) -> Option<&'a [Span<'static>]> {
        let idx = usize::try_from(n.checked_sub(1)?).ok()?;
        rows.get(idx)
            .map(Vec::as_slice)
            .filter(|spans| !spans.is_empty())
    }

    fn old_line(&self, n: i32) -> Option<&'a [Span<'static>]> {
        Self::line(self.old, n)
    }

    fn new_line(&self, n: i32) -> Option<&'a [Span<'static>]> {
        Self::line(self.new, n)
    }
}

/// Layer an optional background tint onto a style (no-op when `None`).
fn bg_style(style: Style, bg: Option<Color>) -> Style {
    match bg {
        Some(bg) => style.bg(bg),
        None => style,
    }
}

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
    let (old_changed, new_changed) = changed_char_ranges(old_text, new_text);
    let old_chars: Vec<char> = old_text.chars().collect();
    let new_chars: Vec<char> = new_text.chars().collect();

    let old_prefix: String = old_chars[..old_changed.start].iter().collect();
    let old_changed_text: String = old_chars[old_changed.clone()].iter().collect();
    let old_suffix: String = old_chars[old_changed.end..].iter().collect();

    let new_prefix: String = new_chars[..new_changed.start].iter().collect();
    let new_changed_text: String = new_chars[new_changed.clone()].iter().collect();
    let new_suffix: String = new_chars[new_changed.end..].iter().collect();

    let removed_spans = vec![
        Span::styled(old_prefix, removed_style),
        Span::styled(
            old_changed_text,
            emphasis_style.fg(removed_style.fg.unwrap_or(ratatui::style::Color::Red)),
        ),
        Span::styled(old_suffix, removed_style),
    ];

    let added_spans = vec![
        Span::styled(new_prefix, added_style),
        Span::styled(
            new_changed_text,
            emphasis_style.fg(added_style.fg.unwrap_or(ratatui::style::Color::Green)),
        ),
        Span::styled(new_suffix, added_style),
    ];

    (removed_spans, added_spans)
}

/// Return the changed character range on each side of a paired diff row.
/// Character indices keep the result safe for UTF-8 source text.
fn changed_char_ranges(old_text: &str, new_text: &str) -> (Range<usize>, Range<usize>) {
    let old_chars: Vec<char> = old_text.chars().collect();
    let new_chars: Vec<char> = new_text.chars().collect();
    let prefix_len = old_chars
        .iter()
        .zip(new_chars.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let suffix_len = old_chars[prefix_len..]
        .iter()
        .rev()
        .zip(new_chars[prefix_len..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (
        prefix_len..old_chars.len().saturating_sub(suffix_len),
        prefix_len..new_chars.len().saturating_sub(suffix_len),
    )
}

/// Preserve syntax foregrounds while layering the diff background and the
/// word-level emphasis over the changed character range.
fn style_syntax_diff_spans(
    spans: &[Span<'static>],
    changed: Range<usize>,
    bg: Option<Color>,
    emphasis: Style,
) -> Vec<Span<'static>> {
    let mut result = Vec::with_capacity(spans.len() + 2);
    let mut global_start = 0usize;

    for span in spans {
        let chars: Vec<char> = span.content.chars().collect();
        let global_end = global_start + chars.len();
        let local_changed_start = changed.start.saturating_sub(global_start).min(chars.len());
        let local_changed_end = changed.end.saturating_sub(global_start).min(chars.len());
        let base_style = bg_style(span.style, bg);

        push_char_span(&mut result, &chars[..local_changed_start], base_style);
        if local_changed_start < local_changed_end {
            push_char_span(
                &mut result,
                &chars[local_changed_start..local_changed_end],
                base_style.patch(emphasis),
            );
        }
        push_char_span(&mut result, &chars[local_changed_end..], base_style);
        global_start = global_end;
    }

    result
}

fn push_char_span(result: &mut Vec<Span<'static>>, chars: &[char], style: Style) {
    if !chars.is_empty() {
        result.push(Span::styled(chars.iter().collect::<String>(), style));
    }
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
pub fn render_diff_lines(
    diff_text: &str,
    styles: UiStyles<'_>,
    width: u16,
    hl: DiffHighlight<'_>,
) -> Vec<Line<'static>> {
    let rows = diff_line_view_refs(diff_text);
    render_rows(&rows, styles, width, hl)
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
    hl: DiffHighlight<'_>,
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
    let head = render_rows_with_gutter(&window.head, gutter_width, styles, width, hl);
    let tail = render_rows_with_gutter(&window.tail, gutter_width, styles, width, hl);
    combine_preview_lines(head, tail, window.omitted, max_rows, truncation_line)
}

fn render_rows(
    rows: &[DiffLineViewRef<'_>],
    styles: UiStyles<'_>,
    width: u16,
    hl: DiffHighlight<'_>,
) -> Vec<Line<'static>> {
    let gutter_width = line_number_width(rows);
    render_rows_with_gutter(rows, gutter_width, styles, width, hl)
}

fn render_rows_with_gutter(
    rows: &[DiffLineViewRef<'_>],
    gutter_width: usize,
    styles: UiStyles<'_>,
    width: u16,
    hl: DiffHighlight<'_>,
) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    // Track the last new-file line number rendered so a hunk boundary can show
    // how many unchanged lines were skipped. `None` at the start of a batch and
    // after each file header means the gap is unknown (e.g. across the omitted
    // middle of a preview window), so no separator is emitted there.
    let mut prev_new_line: Option<i32> = None;

    for row in rows {
        if let DiffLineViewRef::Hunk { new_start, .. } = row
            && let Some(prev) = prev_new_line
        {
            // Saturating so a malformed hunk header (e.g. a negative parsed
            // `new_start`) can never overflow the subtraction.
            let skipped = new_start.saturating_sub(prev).saturating_sub(1);
            if skipped >= 1 {
                result.push(unchanged_separator(skipped, styles));
            }
        }
        result.extend(render_row(*row, gutter_width, styles, width, hl));
        match row {
            DiffLineViewRef::FileHeader { .. } => prev_new_line = None,
            DiffLineViewRef::Context { new_line, .. } | DiffLineViewRef::Added { new_line, .. } => {
                prev_new_line = Some(*new_line);
            }
            DiffLineViewRef::Removed { .. }
            | DiffLineViewRef::Hunk { .. }
            | DiffLineViewRef::RawHunk { .. } => {}
        }
    }

    result
}

/// A `⋯ N unchanged lines` separator shown at a hunk boundary.
fn unchanged_separator(skipped: i32, styles: UiStyles<'_>) -> Line<'static> {
    let plural = if skipped == 1 { "" } else { "s" };
    Line::from(Span::raw(format!("  ⋯ {skipped} unchanged line{plural}")).style(styles.dim_style()))
}

fn render_row(
    row: DiffLineViewRef<'_>,
    gutter_width: usize,
    styles: UiStyles<'_>,
    width: u16,
    hl: DiffHighlight<'_>,
) -> Vec<Line<'static>> {
    let removed_bg = styles.diff_removed_bg();
    let added_bg = styles.diff_added_bg();
    let removed_style = bg_style(Style::new().fg(styles.diff_removed()), removed_bg);
    let added_style = bg_style(Style::new().fg(styles.diff_added()), added_bg);
    let emphasis = Style::new().reversed();

    match row {
        DiffLineViewRef::FileHeader { marker, path } => {
            vec![Line::from(vec![
                Span::raw(format!("  {marker} ")).style(styles.dim_style()),
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
            // Context is unchanged, so it takes syntax tokens (if any) but no
            // diff background tint.
            let content_spans = match hl.new_line(new_line).or_else(|| hl.old_line(old_line)) {
                Some(spans) => spans.to_vec(),
                None => vec![Span::raw(content.to_string()).style(styles.dim_style())],
            };
            render_wrapped_content_line(
                vec![
                    Span::raw(format!("  {old_no} {new_no} ")).style(styles.dim_style()),
                    Span::raw(format!("{BOX_VERTICAL} ")).fg(styles.border()),
                ],
                content_spans,
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
                Span::raw(format!("  {old_no} {blank} ")).style(styles.dim_style()),
                Span::styled(
                    format!("{BOX_VERTICAL} "),
                    bg_style(Style::new().fg(styles.diff_removed()), removed_bg),
                ),
            ];
            let content_spans = if let Some(spans) = hl.old_line(old_line) {
                // Syntax tokens carry the foreground; the diff tint is layered
                // behind them; word emphasis is applied without replacing the
                // token foreground.
                let mut out = vec![Span::styled("-", removed_style)];
                let changed = compare_to
                    .map(|new_text| changed_char_ranges(content, new_text).0)
                    .unwrap_or(0..0);
                out.extend(style_syntax_diff_spans(
                    spans, changed, removed_bg, emphasis,
                ));
                out
            } else if let Some(compare_to) = compare_to {
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
                Span::raw(format!("  {blank} {new_no} ")).style(styles.dim_style()),
                Span::styled(
                    format!("{BOX_VERTICAL} "),
                    bg_style(Style::new().fg(styles.diff_added()), added_bg),
                ),
            ];
            let content_spans = if let Some(spans) = hl.new_line(new_line) {
                let mut out = vec![Span::styled("+", added_style)];
                let changed = compare_to
                    .map(|old_text| changed_char_ranges(old_text, content).1)
                    .unwrap_or(0..0);
                out.extend(style_syntax_diff_spans(spans, changed, added_bg, emphasis));
                out
            } else if let Some(compare_to) = compare_to {
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
    let diff_lines = render_diff_lines(diff_text, styles, content_width, DiffHighlight::default());
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
    // Snap the byte cut to a char boundary so a multi-byte (CJK/emoji) path
    // never panics slicing mid-UTF-8-char (repo String-Slicing rule). Round UP
    // (`ceil`) so the kept tail stays ≤ `keep` bytes and the result never
    // exceeds `max_len`. `tui-ui` is domain-free, so use the std primitive
    // rather than `coco_utils_string`.
    let cut = path.ceil_char_boundary(path.len() - keep);
    format!("...{}", &path[cut..])
}

#[cfg(test)]
#[path = "diff_display.test.rs"]
mod tests;
