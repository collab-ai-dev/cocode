//! Single-row indicator shown beneath the input while a draft is stashed.
//!
//! Renders a subtle dim line with the first ~40 characters of the stashed
//! text so the user remembers what they pushed and can pop it back with the
//! same keybinding (`chat:stash`, Ctrl+S by default).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::i18n::t;
use crate::state::ui::StashedInput;
use coco_tui_ui::style::UiStyles;

const PREVIEW_CHARS: usize = 40;

/// Stash-present indicator: `↺ Stashed: <preview>`.
pub struct StashNotice<'a> {
    stash: &'a StashedInput,
    styles: UiStyles<'a>,
}

impl<'a> StashNotice<'a> {
    pub fn new(stash: &'a StashedInput, styles: UiStyles<'a>) -> Self {
        Self { stash, styles }
    }

    /// Returns `true` when this notice should occupy a row in the layout.
    /// Centralizes the visibility check so callers stay in sync with the
    /// widget's own opinion of when it is renderable.
    pub fn should_display(stash: Option<&StashedInput>) -> bool {
        stash.is_some_and(|s| !s.text.trim().is_empty())
    }

    fn truncated_preview(&self) -> String {
        let single_line: String = self.stash.text.lines().next().unwrap_or("").into();
        if single_line.chars().count() <= PREVIEW_CHARS {
            single_line
        } else {
            let mut s: String = single_line.chars().take(PREVIEW_CHARS).collect();
            s.push('…');
            s
        }
    }
}

impl Widget for StashNotice<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let preview = self.truncated_preview();
        let line = Line::from(vec![
            Span::raw(" ↺ ").fg(self.styles.accent()),
            Span::raw(t!("input.stash_label").to_string()).fg(self.styles.dim()),
            Span::raw(" ").fg(self.styles.dim()),
            Span::raw(preview).fg(self.styles.dim()).italic(),
        ]);
        Paragraph::new(line)
            .style(Style::default())
            .render(area, buf);
    }
}

#[cfg(test)]
#[path = "stash_notice.test.rs"]
mod tests;
