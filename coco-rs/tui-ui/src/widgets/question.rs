//! Pure, domain-free AskUserQuestion widget.

use ratatui::prelude::*;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use crate::style::UiStyles;

const TWO_COL_MIN_WIDTH: u16 = 72;
const OPTION_COL_WIDTH: u16 = 34;
const COL_GAP: u16 = 2;

const CURSOR: &str = "❯ ";
const NO_CURSOR: &str = "  ";
const NAV_ROWS: u16 = 1;

pub enum RowMark {
    Radio { selected: bool, focused: bool },
    Check { checked: bool, focused: bool },
}

impl RowMark {
    fn focused(&self) -> bool {
        match self {
            Self::Radio { focused, .. } | Self::Check { focused, .. } => *focused,
        }
    }
}

pub struct ChoiceRow {
    pub number: usize,
    pub label: String,
    pub description: String,
    pub mark: RowMark,
}

pub struct InputRow {
    pub number: usize,
    pub label: String,
    pub value: String,
    pub selected: bool,
    pub focused: bool,
}

pub struct ActionRow {
    pub number: usize,
    pub label: String,
    pub focused: bool,
}

pub enum QuestionRow {
    Choice(ChoiceRow),
    Input(InputRow),
    Action(ActionRow),
}

impl QuestionRow {
    fn focused(&self) -> bool {
        match self {
            Self::Choice(row) => row.mark.focused(),
            Self::Input(row) => row.focused,
            Self::Action(row) => row.focused,
        }
    }
}

pub struct NavTab {
    pub header: String,
    pub answered: bool,
}

pub struct SubmitNavTab {
    pub focused: bool,
    pub ready: bool,
}

pub struct QuestionNav {
    pub tabs: Vec<NavTab>,
    pub current: usize,
    pub submit: Option<SubmitNavTab>,
}

pub struct QuestionHeader {
    pub title: String,
    pub chip: Option<String>,
    pub nav: Option<QuestionNav>,
}

pub struct QuestionView {
    pub header: QuestionHeader,
    pub body: String,
    pub rows: Vec<QuestionRow>,
    pub submit_review: Option<String>,
    pub preview: Option<String>,
    pub footer_actions: Vec<ActionRow>,
    pub hints: String,
}

struct Composed {
    lines: Vec<Line<'static>>,
    focused: Option<usize>,
    input_cursor: Option<(usize, usize)>,
}

impl QuestionView {
    pub fn desired_height(&self, width: u16, styles: UiStyles<'_>) -> u16 {
        let inner = width.max(1);
        let header = self.header_height(inner);
        let footer = self.footer_height(inner);
        let body_width = if self.two_col(inner) {
            self.option_col_width(inner)
        } else {
            inner
        };
        let body = self
            .compose_body(styles, body_width, !self.two_col(inner))
            .lines
            .len() as u16;
        header.saturating_add(body).saturating_add(footer).max(1)
    }

    fn header_height(&self, width: u16) -> u16 {
        let mut rows = 1;
        if self.header.nav.is_some() {
            rows += NAV_ROWS;
        }
        if let Some(chip) = &self.header.chip
            && !chip.is_empty()
        {
            rows += wrap(&format!("[{chip}]"), width as usize).len() as u16;
        }
        rows
    }

    fn footer_height(&self, width: u16) -> u16 {
        let mut rows = self.footer_actions.len() as u16;
        if !self.footer_actions.is_empty() {
            rows += 1;
        }
        if !self.footer_actions.is_empty() && !self.hints.is_empty() {
            rows += 1;
        }
        if !self.hints.is_empty() {
            rows += wrap(&self.hints, width as usize).len() as u16;
        }
        rows
    }

    fn two_col(&self, width: u16) -> bool {
        self.preview.is_some() && width >= TWO_COL_MIN_WIDTH
    }

    fn option_col_width(&self, width: u16) -> u16 {
        OPTION_COL_WIDTH.min(width.saturating_sub(10).max(1))
    }

    fn compose_body(&self, styles: UiStyles<'_>, width: u16, with_preview: bool) -> Composed {
        let w = width.max(1) as usize;
        let mut lines = Vec::new();
        let mut focused = None;
        let mut input_cursor = None;
        let body = self.submit_review.as_ref().unwrap_or(&self.body);
        for l in wrap(body, w) {
            lines.push(Line::from(l));
        }
        if !self.rows.is_empty() {
            lines.push(Line::from(""));
        }
        let number_width = self.max_row_number().to_string().len();
        for row in &self.rows {
            if row.focused() {
                focused = Some(lines.len());
            }
            if let Some((line_offset, col)) =
                render_question_row(row, styles, number_width, w, &mut lines)
            {
                input_cursor = focused.map(|line| (line + line_offset, col));
            }
        }
        if with_preview && let Some(preview) = &self.preview {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "preview",
                Style::default().fg(styles.dim()),
            )));
            for l in wrap(preview, w) {
                lines.push(Line::from(l));
            }
        }
        Composed {
            lines,
            focused,
            input_cursor,
        }
    }

    fn max_row_number(&self) -> usize {
        self.rows
            .iter()
            .map(|row| match row {
                QuestionRow::Choice(choice) => choice.number,
                QuestionRow::Input(input) => input.number,
                QuestionRow::Action(action) => action.number,
            })
            .chain(self.footer_actions.iter().map(|action| action.number))
            .max()
            .unwrap_or(1)
    }

    fn preview_lines(&self, styles: UiStyles<'_>, width: u16) -> Vec<Line<'static>> {
        let Some(preview) = &self.preview else {
            return Vec::new();
        };
        let mut lines = vec![Line::from(Span::styled(
            "preview",
            Style::default().fg(styles.dim()),
        ))];
        for l in wrap(preview, width.max(1) as usize) {
            lines.push(Line::from(l));
        }
        lines
    }

    pub fn input_cursor_position(&self, area: Rect, styles: UiStyles<'_>) -> Option<Position> {
        if area.width == 0 || area.height == 0 {
            return None;
        }
        let header_h = self.header_height(area.width).min(area.height);
        let footer_h = self
            .footer_height(area.width)
            .min(area.height.saturating_sub(header_h));
        let body_h = area.height.saturating_sub(header_h + footer_h);
        if body_h == 0 {
            return None;
        }
        let body = Rect::new(area.x, area.y + header_h, area.width, body_h);
        let body_width = if self.two_col(body.width) {
            self.option_col_width(body.width)
        } else {
            body.width
        };
        let composed = self.compose_body(styles, body_width, !self.two_col(body.width));
        let (line_idx, col) = composed.input_cursor?;
        let start = scroll_start(&composed, body_h as usize);
        let visible_idx = line_idx.checked_sub(start)?;
        if visible_idx >= body_h as usize {
            return None;
        }
        let x = body.x + (col as u16).min(body_width.saturating_sub(1));
        let y = body.y + visible_idx as u16;
        Some(Position { x, y })
    }
}

pub struct QuestionWidget<'a> {
    view: &'a QuestionView,
    styles: UiStyles<'a>,
}

impl<'a> QuestionWidget<'a> {
    pub fn new(view: &'a QuestionView, styles: UiStyles<'a>) -> Self {
        Self { view, styles }
    }
}

impl Widget for QuestionWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let header_h = self.view.header_height(area.width).min(area.height);
        let footer_h = self
            .view
            .footer_height(area.width)
            .min(area.height.saturating_sub(header_h));
        let body_h = area.height.saturating_sub(header_h + footer_h);
        let [header, body, footer] = area.layout(&Layout::vertical([
            Constraint::Length(header_h),
            Constraint::Length(body_h),
            Constraint::Length(footer_h),
        ]));

        render_header(&self.view.header, self.styles, header, buf);
        if self.view.two_col(body.width) {
            let left_w = self.view.option_col_width(body.width);
            let left = Rect {
                width: left_w,
                ..body
            };
            let preview = Rect {
                x: body.x + left_w + COL_GAP,
                width: body.width.saturating_sub(left_w + COL_GAP),
                ..body
            };
            let composed = self.view.compose_body(self.styles, left.width, false);
            render_scrolled(&composed, left, buf);
            render_top(
                &self.view.preview_lines(self.styles, preview.width),
                preview,
                buf,
            );
        } else {
            let composed = self.view.compose_body(self.styles, body.width, true);
            render_scrolled(&composed, body, buf);
        }
        render_footer(self.view, self.styles, footer, buf);
    }
}

fn render_header(header: &QuestionHeader, styles: UiStyles<'_>, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let mut y = area.y;
    render_separator(styles, area.x, y, area.width, buf);
    y += 1;
    if let Some(nav) = &header.nav
        && y < area.bottom()
    {
        buf.set_line(area.x, y, &nav_line(nav, styles), area.width);
        y += NAV_ROWS;
    }
    if let Some(chip) = &header.chip {
        for line in wrap(&format!("[{chip}]"), area.width as usize) {
            if y >= area.bottom() {
                break;
            }
            buf.set_line(
                area.x,
                y,
                &Line::from(Span::styled(line, Style::default().fg(styles.accent()))),
                area.width,
            );
            y += 1;
        }
    }
}

fn render_footer(view: &QuestionView, styles: UiStyles<'_>, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let mut y = area.y;
    let number_width = view.max_row_number().to_string().len();
    if !view.footer_actions.is_empty() {
        render_separator(styles, area.x, y, area.width, buf);
        y += 1;
    }
    for action in &view.footer_actions {
        if y >= area.bottom() {
            return;
        }
        let mut lines = Vec::new();
        render_action_row(action, styles, number_width, &mut lines);
        if let Some(line) = lines.first() {
            buf.set_line(area.x, y, line, area.width);
        }
        y += 1;
    }
    if !view.footer_actions.is_empty() && !view.hints.is_empty() {
        y += 1;
    }
    for line in wrap(&view.hints, area.width as usize) {
        if y >= area.bottom() {
            return;
        }
        buf.set_line(
            area.x,
            y,
            &Line::from(Span::styled(line, Style::default().fg(styles.dim()))),
            area.width,
        );
        y += 1;
    }
}

fn render_separator(styles: UiStyles<'_>, x: u16, y: u16, width: u16, buf: &mut Buffer) {
    let rule = "─".repeat(width as usize);
    buf.set_line(
        x,
        y,
        &Line::from(Span::styled(rule, Style::default().fg(styles.border()))),
        width,
    );
}

fn render_question_row(
    row: &QuestionRow,
    styles: UiStyles<'_>,
    number_width: usize,
    width: usize,
    lines: &mut Vec<Line<'static>>,
) -> Option<(usize, usize)> {
    match row {
        QuestionRow::Choice(choice) => {
            lines.push(choice_line(choice, styles, number_width));
            for d in wrap(&choice.description, width.saturating_sub(4)) {
                if !d.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!("    {d}"),
                        Style::default().fg(styles.dim()),
                    )));
                }
            }
            None
        }
        QuestionRow::Input(input) => render_input_row(input, styles, number_width, width, lines),
        QuestionRow::Action(action) => {
            render_action_row(action, styles, number_width, lines);
            None
        }
    }
}

fn render_input_row(
    input: &InputRow,
    styles: UiStyles<'_>,
    number_width: usize,
    width: usize,
    lines: &mut Vec<Line<'static>>,
) -> Option<(usize, usize)> {
    let cursor = if input.focused { CURSOR } else { NO_CURSOR };
    let show_placeholder = input.value.is_empty() && !input.focused;
    let value = if show_placeholder {
        input.label.clone()
    } else {
        input.value.clone()
    };
    let value_color = if input.focused {
        styles.accent()
    } else if show_placeholder {
        styles.dim()
    } else {
        styles.text()
    };
    let number = format!("{:>number_width$}. ", input.number);
    let prefix_width = UnicodeWidthStr::width(cursor) + UnicodeWidthStr::width(number.as_str());
    let suffix_width = usize::from(input.focused) + if input.selected { 2 } else { 0 };
    let value_width = width
        .saturating_sub(prefix_width)
        .saturating_sub(suffix_width)
        .max(1);
    let chunks = wrap_display_preserve(&value, value_width);
    let mut cursor_pos = None;
    for (idx, chunk) in chunks.iter().enumerate() {
        let first = idx == 0;
        let last = idx + 1 == chunks.len();
        let line_prefix = if first {
            vec![
                Span::styled(cursor, Style::default().fg(styles.accent())),
                Span::styled(number.clone(), Style::default().fg(styles.dim())),
            ]
        } else {
            vec![Span::raw(" ".repeat(prefix_width))]
        };
        let mut spans = line_prefix;
        spans.push(Span::styled(
            chunk.clone(),
            Style::default().fg(value_color),
        ));
        if last {
            if input.focused {
                spans.push(Span::styled("▌", Style::default().fg(styles.accent())));
                cursor_pos = Some((idx, prefix_width + UnicodeWidthStr::width(chunk.as_str())));
            }
            if input.selected {
                spans.push(Span::styled(" ✔", Style::default().fg(styles.success())));
            }
        }
        lines.push(Line::from(spans));
    }
    cursor_pos
}

fn render_action_row(
    action: &ActionRow,
    styles: UiStyles<'_>,
    number_width: usize,
    lines: &mut Vec<Line<'static>>,
) {
    let cursor = if action.focused { CURSOR } else { NO_CURSOR };
    let color = if action.focused {
        styles.accent()
    } else {
        styles.text()
    };
    lines.push(Line::from(vec![
        Span::styled(cursor, Style::default().fg(styles.accent())),
        Span::styled(
            format!("{:>number_width$}. ", action.number),
            Style::default().fg(styles.dim()),
        ),
        Span::styled(action.label.clone(), Style::default().fg(color)),
    ]));
}

fn choice_line(row: &ChoiceRow, styles: UiStyles<'_>, number_width: usize) -> Line<'static> {
    let focused = row.mark.focused();
    let cursor = if focused { CURSOR } else { NO_CURSOR };
    let label_color = if focused {
        styles.accent()
    } else {
        styles.text()
    };
    let mut spans = vec![
        Span::styled(cursor, Style::default().fg(styles.accent())),
        Span::styled(
            format!("{:>number_width$}. ", row.number),
            Style::default().fg(styles.dim()),
        ),
    ];
    if let RowMark::Check { checked, .. } = row.mark {
        let (glyph, color) = if checked {
            ("[x] ", styles.success())
        } else {
            ("[ ] ", styles.dim())
        };
        spans.push(Span::styled(glyph, Style::default().fg(color)));
    }
    spans.push(Span::styled(
        row.label.clone(),
        Style::default().fg(label_color),
    ));
    if let RowMark::Radio { selected: true, .. } = row.mark {
        spans.push(Span::styled(" ✔", Style::default().fg(styles.success())));
    }
    Line::from(spans)
}

fn nav_line(nav: &QuestionNav, styles: UiStyles<'_>) -> Line<'static> {
    let submit_focused = nav.submit.as_ref().is_some_and(|s| s.focused);
    let focused_idx = if submit_focused {
        nav.tabs.len()
    } else {
        nav.current
    };
    let last = nav.tabs.len();
    let arrow_color = |dim: bool| if dim { styles.dim() } else { styles.text() };
    let selected = || {
        Style::default()
            .bg(styles.selection_bg())
            .fg(styles.selection_fg())
    };
    let mut spans = vec![Span::styled(
        "← ",
        Style::default().fg(arrow_color(focused_idx == 0)),
    )];
    for (i, tab) in nav.tabs.iter().enumerate() {
        let checkbox = if tab.answered { "☒" } else { "☐" };
        let text = format!(" {checkbox} {} ", tab.header);
        let style = if !submit_focused && i == nav.current {
            selected()
        } else {
            Style::default().fg(styles.text())
        };
        spans.push(Span::styled(text, style));
    }
    if let Some(submit) = &nav.submit {
        let mark = if submit.ready { "✔" } else { "☐" };
        let style = if submit.focused {
            selected()
        } else {
            Style::default().fg(styles.text())
        };
        spans.push(Span::styled(format!(" {mark} Submit "), style));
    }
    spans.push(Span::styled(
        " →",
        Style::default().fg(arrow_color(focused_idx == last)),
    ));
    Line::from(spans)
}

fn render_scrolled(composed: &Composed, area: Rect, buf: &mut Buffer) {
    let height = area.height as usize;
    if height == 0 {
        return;
    }
    let start = scroll_start(composed, height);
    for (i, line) in composed.lines[start..].iter().take(height).enumerate() {
        buf.set_line(area.x, area.y + i as u16, line, area.width);
    }
}

fn scroll_start(composed: &Composed, height: usize) -> usize {
    if height == 0 || composed.lines.len() <= height {
        0
    } else {
        let focused = composed.focused.unwrap_or(0);
        focused
            .saturating_sub(height / 2)
            .min(composed.lines.len() - height)
    }
}

fn render_top(lines: &[Line<'static>], area: Rect, buf: &mut Buffer) {
    for (i, line) in lines.iter().take(area.height as usize).enumerate() {
        buf.set_line(area.x, area.y + i as u16, line, area.width);
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let mut line = String::new();
        let mut line_w = 0usize;
        for word in raw.split_whitespace() {
            let word_w = UnicodeWidthStr::width(word);
            let sep = usize::from(line_w > 0);
            if line_w > 0 && line_w + sep + word_w > width {
                out.push(std::mem::take(&mut line));
                line_w = 0;
            }
            if word_w > width {
                if line_w > 0 {
                    out.push(std::mem::take(&mut line));
                    line_w = 0;
                }
                for ch in word.chars() {
                    let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                    if line_w + cw > width {
                        out.push(std::mem::take(&mut line));
                        line_w = 0;
                    }
                    line.push(ch);
                    line_w += cw;
                }
                continue;
            }
            if line_w > 0 {
                line.push(' ');
                line_w += 1;
            }
            line.push_str(word);
            line_w += word_w;
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn wrap_display_preserve(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut line = String::new();
    let mut line_w = 0usize;
    for ch in text.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if line_w > 0 && line_w + cw > width {
            out.push(std::mem::take(&mut line));
            line_w = 0;
        }
        line.push(ch);
        line_w += cw;
    }
    out.push(line);
    out
}

#[cfg(test)]
#[path = "question.test.rs"]
mod tests;
