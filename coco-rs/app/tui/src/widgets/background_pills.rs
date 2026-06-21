//! Background pills bar — flat `@name` list for backgrounded subagents.
//!
//! A flat `@{name}` list joined by a single space, with idle/selected/viewed
//! states picking different styles (dim, inverse, colored). No brackets,
//! no glyph, no elapsed segment, no completion-flash window — backgrounded
//! tasks remain in the row indefinitely (the filter is `isBackgroundTask`,
//! not "running within last 5 s").
//!
//! Width budgeting per pill: `stringWidth("@" + name) + (i > 0 ? 1 : 0)`.
//!
//! DIVERGE: three pieces are deliberately unimplemented and tracked here
//! so a future port can pick them up cleanly:
//! - **Leader pill**: the upstream implementation always prepends a leader
//!   row to the pills list. coco-rs has no "main" leader concept on
//!   the pills bar yet — the leader's status surfaces through the
//!   status indicator instead.
//! - **Summary pill**: in non-teammate mode the upstream shows a single
//!   aggregated label ("3 running"). coco-rs always shows the per-task
//!   pill list and relies on overflow tail for compactness.
//! - **Horizontal scrolling**: the upstream scrolls the pill row when it
//!   overflows and tracks a focused pill index. coco-rs renders
//!   `[+N more]` as a static tail instead — simpler, no focus state,
//!   but no way to inspect overflowed pills without entering the
//!   Teammates view.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::state::AppState;
use crate::state::session::SubagentStatus;
use coco_tui_ui::style::UiStyles;

/// Reserve enough columns for a trailing `[+N more]` tail before we
/// commit to painting another pill.
const OVERFLOW_RESERVE: usize = 12;

/// Single-space gap between adjacent pills.
const PILL_GAP: &str = " ";

/// Prefix marker — renders `@{name}` literally.
const PILL_PREFIX: &str = "@";

/// Per-pill view-model. `label` is the agent's display name (the
/// `@` prefix is added by the renderer). `is_idle` is true for
/// backgrounded subagents whose status has reached a terminal value
/// (renders dimmed).
#[derive(Debug, Clone)]
pub(crate) struct PillEntry<'a> {
    pub(crate) label: &'a str,
    pub(crate) is_idle: bool,
    /// Agent badge color; a running pill renders in its color when set.
    pub(crate) color: Option<coco_types::AgentColorName>,
}

/// Borrowed view-model. Pills follow insertion order from
/// `state.session.subagents` (which is itself FIFO by spawn time).
#[derive(Debug, Clone, Default)]
pub(crate) struct BackgroundPillsView<'a> {
    pub(crate) pills: Vec<PillEntry<'a>>,
}

impl BackgroundPillsView<'_> {
    pub(crate) fn is_empty(&self) -> bool {
        self.pills.is_empty()
    }
}

/// Project `AppState` into the pills view-model.
///
/// Every subagent flagged `is_backgrounded` participates, regardless
/// of run status — terminal ones render with `is_idle = true`.
pub(crate) fn build_view(state: &AppState) -> BackgroundPillsView<'_> {
    let pills = state
        .session
        .subagents
        .iter()
        .filter(|a| a.is_backgrounded)
        .map(|a| PillEntry {
            label: a.description.as_str(),
            is_idle: !matches!(a.status, SubagentStatus::Running),
            color: a.color,
        })
        .collect();
    BackgroundPillsView { pills }
}

/// Render `@a @b @c [+N more]` — single-space-separated, with an
/// overflow tail when the row would exceed the available width.
pub(crate) struct BackgroundPills<'a> {
    view: &'a BackgroundPillsView<'a>,
    styles: UiStyles<'a>,
}

impl<'a> BackgroundPills<'a> {
    pub(crate) fn new(view: &'a BackgroundPillsView<'a>, styles: UiStyles<'a>) -> Self {
        Self { view, styles }
    }
}

impl Widget for BackgroundPills<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 || self.view.is_empty() {
            return;
        }

        let max_w = usize::from(area.width);
        // Worst case is 3 spans per pill (gap + prefix + label) plus
        // 2 spans for the overflow tail.
        let mut spans: Vec<Span<'_>> = Vec::with_capacity(self.view.pills.len() * 3 + 2);
        let mut used = 0usize;
        let mut overflow = 0usize;

        for (i, pill) in self.view.pills.iter().enumerate() {
            let separator_w = if i == 0 { 0 } else { PILL_GAP.width() };
            let pill_w = PILL_PREFIX.width() + pill.label.width();
            let proposed = used + separator_w + pill_w;
            // Reserve room for `[+N more]` if there is at least one
            // more pill we might not be able to paint.
            if proposed + OVERFLOW_RESERVE > max_w && i + 1 < self.view.pills.len() {
                overflow = self.view.pills.len() - i;
                break;
            }
            if proposed > max_w {
                overflow = self.view.pills.len() - i;
                break;
            }
            if i > 0 {
                spans.push(Span::raw(PILL_GAP));
            }
            // Idle pills are dim; running pills render in the agent's
            // assigned color when set, else the default text color.
            let color = if pill.is_idle {
                self.styles.dim()
            } else {
                pill.color
                    .map(crate::widgets::suggestion_popup::agent_color_to_ratatui)
                    .unwrap_or_else(|| self.styles.text())
            };
            spans.push(Span::raw(PILL_PREFIX).fg(color));
            spans.push(Span::raw(pill.label).fg(color));
            used = proposed;
        }
        if overflow > 0 {
            spans.push(Span::raw(PILL_GAP).fg(self.styles.dim()));
            spans.push(Span::raw(format!("[+{overflow} more]")).fg(self.styles.dim()));
        }

        Paragraph::new(Line::from(spans)).render(area, buf);
    }
}

#[cfg(test)]
#[path = "background_pills.test.rs"]
mod tests;
