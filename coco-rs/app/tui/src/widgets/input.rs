//! Input composer widget.
//!
//! Owns the prompt indicator, placeholder, prefix stripping, mode title, and
//! streaming/queued-input presentation. Cursor placement reuses
//! [`InputRenderModel`] so the rendered text and cursor math stay aligned.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::i18n::t;
use crate::state::ui::InputState;
use crate::state::ui::PromptMode;
use coco_tui_ui::style::UiStyles;

/// Pure input presentation data shared by the input widget and cursor logic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputRenderModel {
    pub(crate) prompt_mode: PromptMode,
    pub(crate) prefix_consumed: usize,
    pub(crate) display_text: String,
    pub(crate) inline_hint: Option<String>,
    pub(crate) inline_ghost: Option<InlineGhostRender>,
    pub(crate) title: String,
    pub(crate) command_palette_filter: Option<String>,
    pub(crate) is_placeholder: bool,
    pub(crate) is_streaming: bool,
    /// Cursor's display line within `display_text` (0-based; counts hard `\n`
    /// breaks). Drives the multi-line composer's cursor row + scroll.
    pub(crate) cursor_row: usize,
    /// Cursor's display column within its line (excludes the `❯ ` indicator).
    pub(crate) cursor_col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlineGhostRender {
    pub(crate) byte_pos: usize,
    pub(crate) text: String,
}

impl InputRenderModel {
    pub(crate) fn build(
        input: &InputState,
        is_streaming: bool,
        prompt_suggestion: Option<&str>,
        has_editable_queue: bool,
        command_palette_filter: Option<&str>,
    ) -> Self {
        let prompt_mode = if is_streaming {
            PromptMode::Normal
        } else {
            input.prompt_mode()
        };
        let is_empty = input.is_empty();

        // Match submit-time prefix stripping: drop the leading mode char plus
        // one optional space so display text equals what the engine receives.
        let prefix_consumed = if is_empty || prompt_mode == PromptMode::Normal {
            0
        } else {
            let body = &input.text()[1..];
            1 + if body.starts_with(' ') { 1 } else { 0 }
        };

        let (display_text, inline_hint, is_placeholder, command_palette_filter) =
            if let Some(filter) = command_palette_filter {
                (format!("/{filter}"), None, false, Some(filter.to_string()))
            } else if is_empty {
                if has_editable_queue {
                    // Mirrors TS `usePromptInputPlaceholder`: an empty composer
                    // with queued messages hints how to recall them.
                    (t!("input.placeholder_queued").to_string(), None, true, None)
                } else if let Some(suggestion) = prompt_suggestion {
                    (suggestion.to_string(), None, true, None)
                } else {
                    (String::new(), None, false, None)
                }
            } else {
                (
                    input.text()[prefix_consumed..].to_string(),
                    input.inline_hint.clone(),
                    false,
                    None,
                )
            };
        let inline_ghost = if is_placeholder || command_palette_filter.is_some() {
            None
        } else {
            input.active_inline_ghost().and_then(|ghost| {
                let byte_pos = ghost.insert_position.checked_sub(prefix_consumed)?;
                (byte_pos <= display_text.len()).then(|| InlineGhostRender {
                    byte_pos,
                    text: ghost.text.clone(),
                })
            })
        };

        // No queue/streaming title label: the input box stays clean while a
        // turn runs (TS parity). The single queued-input affordance is the
        // dimmed footer strip (`QueueStatusWidget`), shown only once something
        // is actually queued.
        let title = if !is_streaming && prompt_mode != PromptMode::Normal {
            format!(" {} ", t!(prompt_mode.title_i18n_key()))
        } else {
            String::new()
        };

        // Cursor's (row, col) within the displayed text — placeholder / palette
        // states park it at the origin (their cursor is handled specially).
        let (cursor_row, cursor_col) = if is_placeholder || command_palette_filter.is_some() {
            (0, 0)
        } else {
            let cursor_byte = input
                .textarea
                .cursor()
                .saturating_sub(prefix_consumed)
                .min(display_text.len());
            let before = &display_text[..cursor_byte];
            let row = before.matches('\n').count();
            let line_start = before.rfind('\n').map_or(0, |i| i + 1);
            let col = UnicodeWidthStr::width(&before[line_start..]);
            (row, col)
        };

        Self {
            prompt_mode,
            prefix_consumed,
            display_text,
            inline_hint,
            inline_ghost,
            title,
            command_palette_filter,
            is_placeholder,
            is_streaming,
            cursor_row,
            cursor_col,
        }
    }
}

/// Active reverse-i-search projection for the composer footer.
#[derive(Debug, Clone, Copy)]
pub struct HistorySearchView<'a> {
    /// The query typed so far.
    pub query: &'a str,
    /// Whether the query currently previews a match in the composer.
    pub matched: bool,
}

/// Input widget with prompt-mode indicator and placeholder handling.
pub struct InputWidget<'a> {
    input: &'a InputState,
    styles: UiStyles<'a>,
    focused: bool,
    is_streaming: bool,
    prompt_suggestion: Option<&'a str>,
    has_editable_queue: bool,
    command_palette_filter: Option<&'a str>,
    history_search: Option<HistorySearchView<'a>>,
}

impl<'a> InputWidget<'a> {
    pub fn new(input: &'a InputState, styles: UiStyles<'a>) -> Self {
        Self {
            input,
            styles,
            focused: true,
            is_streaming: false,
            prompt_suggestion: None,
            has_editable_queue: false,
            command_palette_filter: None,
            history_search: None,
        }
    }

    pub fn history_search(mut self, view: Option<HistorySearchView<'a>>) -> Self {
        self.history_search = view;
        self
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    pub fn streaming(mut self, streaming: bool) -> Self {
        self.is_streaming = streaming;
        self
    }

    pub fn prompt_suggestion(mut self, suggestion: Option<&'a str>) -> Self {
        self.prompt_suggestion = suggestion;
        self
    }

    pub fn has_editable_queue(mut self, has_queue: bool) -> Self {
        self.has_editable_queue = has_queue;
        self
    }

    pub fn command_palette_filter(mut self, filter: Option<&'a str>) -> Self {
        self.command_palette_filter = filter;
        self
    }

    fn model(&self) -> InputRenderModel {
        InputRenderModel::build(
            self.input,
            self.is_streaming,
            self.prompt_suggestion,
            self.has_editable_queue,
            self.command_palette_filter,
        )
    }
}

impl Widget for InputWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let model = self.model();
        let border_color = if self.focused {
            self.styles.focused_border()
        } else {
            self.styles.border()
        };

        let indicator = match (model.is_streaming, model.prompt_mode) {
            (true, _) => Span::styled("~ ", Style::default().fg(self.styles.warning())),
            (false, PromptMode::Bash) => {
                Span::styled("! ", Style::default().fg(self.styles.accent())).bold()
            }
            (false, PromptMode::Normal) => {
                Span::styled("❯ ", Style::default().fg(self.styles.primary()))
            }
        };
        let text_style = if model.is_placeholder {
            Style::default().fg(self.styles.dim())
        } else {
            Style::default().fg(self.styles.text())
        };
        let lines: Vec<Line> = if model.display_text.contains('\n') {
            // Multi-line composer: one row per hard line break, scrolled to keep
            // the cursor visible (mirrors TS, whose TextInput grows with content
            // so recalled multi-message edits show on separate rows). Row 0 wears
            // the indicator; continuation rows align under it. Inline ghost/hint
            // are single-line affordances and are omitted here.
            let content_rows = area.height.saturating_sub(2).max(1) as usize;
            let segments: Vec<&str> = model.display_text.split('\n').collect();
            let scroll = scroll_offset(model.cursor_row, segments.len(), content_rows);
            segments
                .iter()
                .enumerate()
                .skip(scroll)
                .take(content_rows)
                .map(|(idx, seg)| {
                    let gutter = if idx == 0 {
                        indicator.clone()
                    } else {
                        Span::raw("  ")
                    };
                    Line::from(vec![gutter, Span::styled((*seg).to_string(), text_style)])
                })
                .collect()
        } else {
            let mut spans = vec![indicator];
            if let Some(ghost) = model.inline_ghost.as_ref() {
                let split = ghost.byte_pos.min(model.display_text.len());
                let before = model.display_text[..split].to_string();
                let after = model.display_text[split..].to_string();
                spans.push(Span::styled(before, text_style));
                spans.push(Span::styled(
                    ghost.text.clone(),
                    Style::default().fg(self.styles.dim()),
                ));
                spans.push(Span::styled(after, text_style));
            } else {
                spans.push(Span::styled(model.display_text.clone(), text_style));
            }
            if let Some(hint) = model.inline_hint.as_ref() {
                spans.push(Span::styled(
                    hint.clone(),
                    Style::default().fg(self.styles.dim()),
                ));
            }
            vec![Line::from(spans)]
        };
        let mut block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .title(model.title)
            .border_style(Style::default().fg(border_color));
        if let Some(view) = self.history_search {
            block = block.title_bottom(self.history_search_footer_line(view));
        }
        Paragraph::new(lines).block(block).render(area, buf);
    }
}

/// First visible logical row so the cursor row stays within a `content_rows`
/// window. Shared by the composer render and the cursor placement so they
/// agree on scroll. `content_rows` is assumed ≥ 1.
pub(crate) fn scroll_offset(cursor_row: usize, total_rows: usize, content_rows: usize) -> usize {
    if total_rows <= content_rows {
        return 0;
    }
    let max_scroll = total_rows - content_rows;
    cursor_row.saturating_sub(content_rows - 1).min(max_scroll)
}

impl InputWidget<'_> {
    /// Shell-style `reverse-i-search: <query>` footer rendered on the
    /// composer's bottom border while a Ctrl+R search is active.
    fn history_search_footer_line(&self, view: HistorySearchView<'_>) -> Line<'static> {
        let mut spans = vec![
            Span::styled(
                t!("input.reverse_search_label").to_string(),
                Style::default().fg(self.styles.dim()),
            ),
            Span::styled(
                view.query.to_string(),
                Style::default().fg(self.styles.accent()),
            ),
        ];
        if view.matched {
            spans.push(Span::styled(
                format!("  {}", t!("input.reverse_search_hint")),
                Style::default().fg(self.styles.dim()),
            ));
        } else if !view.query.is_empty() {
            spans.push(Span::styled(
                format!("  {}", t!("input.reverse_search_no_match")),
                Style::default().fg(self.styles.warning()),
            ));
        }
        Line::from(spans)
    }
}

#[cfg(test)]
#[path = "input.test.rs"]
mod tests;
