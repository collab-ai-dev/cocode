//! Agent-switcher rail — the inline vertical list above the composer that
//! lists `◯ main` plus every running subagent. It is **always visible** while
//! subagents run; focus (reached with `Shift+↑`) only changes its styling, not
//! its layout. The focused row carries a `❯` cursor; the currently-viewed
//! agent carries a `◀` marker. Pressing `Enter` on a row switches the
//! read-only agent overlay to that agent (or back to main); `x` stops it.
//!
//! View-model in, ratatui out: [`build_view`] projects `AppState`, the
//! [`AgentSwitcher`] widget paints. Agent rows reuse the shared
//! `agent_color_to_ratatui` palette so the type badge matches the activity
//! panel and the background-tasks dialog.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::i18n::t;
use crate::presentation::layout::truncate_to_width;
use crate::state::AppState;
use crate::state::FocusTarget;
use coco_tui_ui::style::UiStyles;

/// `◯` for the synthetic main row, `⏺` for an agent row.
const MAIN_GLYPH: &str = "◯";
const AGENT_GLYPH: &str = "⏺";
/// Marks the row whose conversation is currently open in the overlay.
const VIEWING_MARK: &str = " ◀";

/// One rail row. The synthetic main row has `agent_type == None`.
#[derive(Debug, Clone)]
pub(crate) struct SwitcherRow<'a> {
    pub(crate) agent_type: Option<&'a str>,
    /// `main`, or the agent's latest activity / description.
    pub(crate) label: &'a str,
    pub(crate) color: Option<coco_types::AgentColorName>,
}

/// Borrowed view-model for the rail.
#[derive(Debug, Clone, Default)]
pub(crate) struct AgentSwitcherView<'a> {
    pub(crate) rows: Vec<SwitcherRow<'a>>,
    /// Selected row index; only meaningful when `focused`.
    pub(crate) selected: usize,
    /// Whether the rail currently holds focus (`Shift+↑` activated it).
    pub(crate) focused: bool,
    /// Row index whose conversation is open in the overlay, if any.
    pub(crate) viewing: Option<usize>,
}

impl AgentSwitcherView<'_> {
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Rows the rail occupies (one per agent). `0` when there are no agents.
    pub(crate) fn row_count(&self) -> u16 {
        self.rows.len() as u16
    }
}

/// Project `AppState` into the rail view-model. Row `0` is always `◯ main`;
/// rows `1..` are the running subagents in spawn order.
pub(crate) fn build_view(state: &AppState) -> AgentSwitcherView<'_> {
    let agents = state.session.switcher_agents();
    let rows: Vec<SwitcherRow<'_>> = agents
        .iter()
        .map(|a| {
            let label = a
                .recent_activities
                .last()
                .map(|act| act.summary.as_deref().unwrap_or(act.tool_name.as_str()))
                .filter(|s| !s.is_empty())
                .unwrap_or(a.description.as_str());
            SwitcherRow {
                agent_type: Some(a.agent_type.as_str()),
                label,
                color: a.color,
            }
        })
        .collect();
    let focused = state.ui.focus == FocusTarget::AgentSwitcher;
    let selected = state
        .ui
        .agent_switcher_selected
        .min(rows.len().saturating_sub(1));
    let viewing = state
        .session
        .viewing_agent_id
        .as_deref()
        .and_then(|id| agents.iter().position(|a| a.agent_id == id));
    AgentSwitcherView {
        rows,
        selected,
        focused,
        viewing,
    }
}

/// Paints the rail, one row per line.
pub(crate) struct AgentSwitcher<'a> {
    view: &'a AgentSwitcherView<'a>,
    styles: UiStyles<'a>,
}

impl<'a> AgentSwitcher<'a> {
    pub(crate) fn new(view: &'a AgentSwitcherView<'a>, styles: UiStyles<'a>) -> Self {
        Self { view, styles }
    }

    fn row_line(&self, idx: usize, row: &SwitcherRow<'_>, width: usize) -> Line<'static> {
        let selected = self.view.focused && idx == self.view.selected;
        let viewing = self.view.viewing == Some(idx);
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(
            if selected { "❯ " } else { "  " }.to_string(),
            Style::default().fg(self.styles.accent()),
        ));

        // Glyph + (agent type) + label. The main row is dim when unfocused.
        let base = if selected {
            self.styles.accent()
        } else if self.view.focused {
            self.styles.text()
        } else {
            self.styles.dim()
        };
        if let Some(agent_type) = row.agent_type {
            let color = row
                .color
                .map(crate::widgets::suggestion_popup::agent_color_to_ratatui)
                .unwrap_or(base);
            spans.push(Span::styled(
                format!("{AGENT_GLYPH} "),
                Style::default().fg(color),
            ));
            spans.push(Span::styled(
                format!("{agent_type}  "),
                Style::default().fg(if selected {
                    self.styles.accent()
                } else {
                    color
                }),
            ));
            spans.push(Span::styled(
                truncate_to_width(row.label, 48),
                Style::default().fg(base),
            ));
        } else {
            spans.push(Span::styled(
                format!("{MAIN_GLYPH} "),
                Style::default().fg(base),
            ));
            spans.push(Span::styled(
                row.label.to_string(),
                Style::default().fg(base),
            ));
        }
        if viewing {
            spans.push(Span::styled(
                VIEWING_MARK.to_string(),
                Style::default().fg(self.styles.accent()),
            ));
        }

        // Right-align a hint on the first row: focused vs collapsed.
        if idx == 0 {
            let hint = if self.view.focused {
                t!("switcher.hint_focused").to_string()
            } else {
                t!("switcher.hint_collapsed").to_string()
            };
            let used: usize = spans.iter().map(|s| s.content.width()).sum();
            let pad = width
                .saturating_sub(used)
                .saturating_sub(hint.width())
                .saturating_sub(1);
            if pad > 0 {
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(hint, Style::default().fg(self.styles.dim())));
            }
        }
        Line::from(spans)
    }
}

impl Widget for AgentSwitcher<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 || self.view.is_empty() {
            return;
        }
        let width = usize::from(area.width);
        let lines: Vec<Line<'static>> = self
            .view
            .rows
            .iter()
            .enumerate()
            .take(area.height as usize)
            .map(|(i, row)| self.row_line(i, row, width))
            .collect();
        Paragraph::new(lines).render(area, buf);
    }
}

#[cfg(test)]
#[path = "agent_switcher.test.rs"]
mod tests;
