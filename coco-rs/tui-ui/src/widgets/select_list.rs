//! Generic, domain-free single-select list — the reusable core shared by
//! pickers (theme, and future menus). Mirrors claude-code's `Select<T>`: the
//! caller supplies items plus the focused index, and this renders a `❯` focus
//! cursor, an optional `✔` active marker, optional numbering, and a scroll
//! window that keeps the focused row visible. No `AppState`, no i18n — just
//! values in, themed `Line`s out, so any command can reuse it.

use std::ops::Range;

use ratatui::prelude::*;

use crate::style::UiStyles;

/// One row in a select list. Intentionally value-less: the caller maps the
/// chosen index back to its own data, which keeps this widget domain-free.
#[derive(Debug, Clone)]
pub struct SelectItem {
    /// Primary text shown for the row.
    pub label: String,
    /// Optional trailing dim text (e.g. a description or hint).
    pub secondary: Option<String>,
    /// Whether this row is the currently-applied value (renders a `✔`).
    pub active: bool,
}

impl SelectItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            secondary: None,
            active: false,
        }
    }

    pub fn with_active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn with_secondary(mut self, secondary: impl Into<String>) -> Self {
        self.secondary = Some(secondary.into());
        self
    }
}

/// Presentation knobs for [`render_select_list`].
#[derive(Debug, Clone)]
pub struct SelectListStyle {
    /// Prefix each row with `"{n}. "` (1-based), mirroring TS `Select`.
    pub numbered: bool,
    /// Max rows shown at once; longer lists scroll to keep the focus visible.
    pub visible_count: usize,
}

impl Default for SelectListStyle {
    fn default() -> Self {
        Self {
            numbered: true,
            visible_count: 12,
        }
    }
}

const CURSOR: &str = "❯ ";
const NO_CURSOR: &str = "  ";
const ACTIVE_MARK: &str = "✔";

/// Render a single-select list to owned `Line`s. `selected` is clamped into
/// range. The focused row carries the `accent` cursor and a bold label; the
/// active (currently-applied) row is marked with a `success`-colored `✔`.
pub fn render_select_list(
    items: &[SelectItem],
    selected: usize,
    style: &SelectListStyle,
    styles: UiStyles<'_>,
) -> Vec<Line<'static>> {
    if items.is_empty() {
        return Vec::new();
    }
    let selected = selected.min(items.len() - 1);
    let window = scroll_window(items.len(), selected, style.visible_count.max(1));
    let number_width = if style.numbered {
        items.len().to_string().len()
    } else {
        0
    };

    window
        .map(|i| {
            let item = &items[i];
            let focused = i == selected;
            let mut spans: Vec<Span<'static>> = Vec::new();

            spans.push(Span::styled(
                if focused { CURSOR } else { NO_CURSOR }.to_string(),
                Style::default().fg(styles.accent()),
            ));

            if style.numbered {
                spans.push(Span::styled(
                    format!("{:>width$}. ", i + 1, width = number_width),
                    Style::default().fg(styles.dim()),
                ));
            }

            // TS `ListItem` row colors: the active (applied) row is `success`,
            // the focused row is `accent`, everything else is body text. No
            // bold — emphasis comes from color + the cursor glyph.
            let label_color = if item.active {
                styles.success()
            } else if focused {
                styles.accent()
            } else {
                styles.text()
            };
            spans.push(Span::styled(
                item.label.clone(),
                Style::default().fg(label_color),
            ));

            if item.active {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    ACTIVE_MARK.to_string(),
                    Style::default().fg(styles.success()),
                ));
            }

            if let Some(secondary) = &item.secondary {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    secondary.clone(),
                    Style::default().fg(styles.dim()),
                ));
            }

            Line::from(spans)
        })
        .collect()
}

/// Indices to render so `selected` stays visible within `visible` rows.
fn scroll_window(len: usize, selected: usize, visible: usize) -> Range<usize> {
    if len <= visible {
        return 0..len;
    }
    let half = visible / 2;
    let start = selected.saturating_sub(half).min(len - visible);
    start..start + visible
}

#[cfg(test)]
#[path = "select_list.test.rs"]
mod tests;
