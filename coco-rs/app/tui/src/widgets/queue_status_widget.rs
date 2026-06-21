//! Queued-commands preview strip.
//!
//! Renders `SessionState::queued_commands` — messages the user typed while the
//! agent was busy that will process at the next turn boundary — as a multi-line
//! dimmed list, one `❯ <preview>` per command. Mirrors TS
//! `PromptInputQueuedCommands` (a `flexDirection="column"` box of dimmed user
//! messages). Capped at [`MAX_ROWS`]; the remainder collapses into a final
//! "+N more" summary row. The "Press up to edit queued messages" affordance
//! lives in the input placeholder (TS `usePromptInputPlaceholder`), not here.

use std::collections::VecDeque;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::i18n::t;
use crate::state::session::QueuedCommandDisplay;
use coco_tui_ui::style::UiStyles;

/// Max preview rows before the tail collapses into a single "+N more" row.
const MAX_ROWS: usize = 6;

pub struct QueueStatusWidget<'a> {
    queued: &'a VecDeque<QueuedCommandDisplay>,
    styles: UiStyles<'a>,
}

impl<'a> QueueStatusWidget<'a> {
    pub fn new(queued: &'a VecDeque<QueuedCommandDisplay>, styles: UiStyles<'a>) -> Self {
        Self { queued, styles }
    }

    /// Rows this strip occupies: one blank top-margin row (TS `marginTop={1}`,
    /// separating the queue from the subagent/spinner panel above) plus one row
    /// per queued command, capped at [`MAX_ROWS`] (the cap row doubles as the
    /// "+N more" summary). Zero when the queue is empty. The sizing pass in
    /// `surface::viewport` reads this so it reserves the matching height.
    pub fn height(queued: &VecDeque<QueuedCommandDisplay>) -> u16 {
        if queued.is_empty() {
            0
        } else {
            (queued.len().min(MAX_ROWS) + 1) as u16
        }
    }
}

impl Widget for QueueStatusWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || self.queued.is_empty() {
            return;
        }
        // Leading blank row = TS `marginTop={1}` separating the queue from the
        // subagent/spinner panel above; the previews paint from the next row.
        let area = Rect {
            y: area.y.saturating_add(1),
            height: area.height.saturating_sub(1),
            ..area
        };
        if area.height == 0 {
            return;
        }
        let dim = Style::default().fg(self.styles.dim());
        let chevron = Style::default().fg(self.styles.accent());
        let total = self.queued.len();
        let overflow = total > MAX_ROWS;
        // On overflow keep MAX_ROWS-1 previews + one summary row.
        let shown = if overflow { MAX_ROWS - 1 } else { total };
        // Width budget for the preview text after the "  ❯ " prefix.
        let text_width = (area.width as usize).saturating_sub(4).max(8);

        let mut lines: Vec<Line> = Vec::with_capacity(shown + usize::from(overflow));
        for cmd in self.queued.iter().take(shown) {
            lines.push(Line::from(vec![
                Span::styled("  ❯ ", chevron),
                Span::styled(truncate_preview(&cmd.preview, text_width), dim),
            ]));
        }
        if overflow {
            lines.push(Line::from(Span::styled(
                t!("queue_status.more", count = total - shown).to_string(),
                dim,
            )));
        }
        Paragraph::new(lines).render(area, buf);
    }
}

/// Collapse a queued command to a single-line preview and cap its width.
fn truncate_preview(text: &str, max_chars: usize) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let mut out: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
#[path = "queue_status_widget.test.rs"]
mod tests;
