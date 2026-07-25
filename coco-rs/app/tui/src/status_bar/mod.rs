//! App-owned status bar model, custom statusLine runtime, and rendering.

mod builtin;
pub(crate) mod runtime;
mod widget;

use unicode_width::UnicodeWidthStr;

use crate::state::AppState;
use crate::state::ExitKey;

pub(crate) use runtime::StatusLineRuntime;
pub(crate) use runtime::StatusLineUpdate;
pub(crate) use widget::StatusBarWidget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusTone {
    Primary,
    Dim,
    Border,
    Warning,
    Accent,
    Plan,
    Error,
    Success,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusSpan {
    pub(crate) text: String,
    pub(crate) tone: StatusTone,
    pub(crate) bold: bool,
}

impl StatusSpan {
    pub(crate) fn new(text: impl Into<String>, tone: StatusTone) -> Self {
        Self {
            text: text.into(),
            tone,
            bold: false,
        }
    }

    pub(crate) fn bold(text: impl Into<String>, tone: StatusTone) -> Self {
        Self {
            text: text.into(),
            tone,
            bold: true,
        }
    }
}

/// Drop order when the built-in bar does not fit the terminal width.
///
/// Narrow terminals lose whole items rather than being clipped mid-item by the
/// paragraph renderer, which is what a half-drawn `ctx 8` or a truncated branch
/// name used to look like. Lowest priority drops first; within one priority the
/// rightmost item drops first, so the reading order of whatever survives never
/// changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StatusPriority {
    /// Affordances and badges: cycle hints, MCP/LSP, transcript counts. The
    /// user loses a reminder, not information.
    Ambient,
    /// Session vitals: spend, cache, working directory, task pill.
    Vitals,
    /// Never dropped: model identity, context usage, permission mode, and any
    /// warning. Losing these silently changes what the user believes is true.
    Essential,
}

/// One droppable unit of a built-in status line.
///
/// Items own their own leading separator, so filtering an item out leaves the
/// remaining spans reading correctly with no separator fixup. That holds
/// because the first item on every line is [`StatusPriority::Essential`] and
/// therefore always survives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusItem {
    pub(crate) spans: Vec<StatusSpan>,
    pub(crate) priority: StatusPriority,
}

impl StatusItem {
    pub(crate) fn new(priority: StatusPriority, spans: Vec<StatusSpan>) -> Self {
        Self { spans, priority }
    }

    fn width(&self) -> usize {
        self.spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.text.as_str()))
            .sum()
    }
}

impl<'a> IntoIterator for &'a StatusItem {
    type Item = &'a StatusSpan;
    type IntoIter = std::slice::Iter<'a, StatusSpan>;

    fn into_iter(self) -> Self::IntoIter {
        self.spans.iter()
    }
}

/// Spans of the widest prefix-preserving subset of `items` that fits `width`
/// display columns.
///
/// If even the essential items overflow, they are returned anyway — there is
/// nothing useful left to drop, and the renderer clips.
pub(crate) fn fit_status_items(items: &[StatusItem], width: usize) -> Vec<StatusSpan> {
    let mut kept: Vec<bool> = vec![true; items.len()];
    for priority in [StatusPriority::Ambient, StatusPriority::Vitals] {
        for index in (0..items.len()).rev() {
            if kept_width(items, &kept) <= width {
                break;
            }
            if items[index].priority == priority {
                kept[index] = false;
            }
        }
    }
    items
        .iter()
        .zip(kept)
        .filter(|(_, keep)| *keep)
        .flat_map(|(item, _)| item.spans.iter().cloned())
        .collect()
}

fn kept_width(items: &[StatusItem], kept: &[bool]) -> usize {
    items
        .iter()
        .zip(kept)
        .filter(|(_, keep)| **keep)
        .map(|(item, _)| item.width())
        .sum()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StatusBarView {
    ExitPrompt { key: ExitKey, text: String },
    Custom { line: String },
    BuiltIn { lines: Vec<Vec<StatusItem>> },
}

pub(crate) use builtin::background_pill_label;

pub(crate) fn status_bar_view(state: &AppState) -> StatusBarView {
    if let Some(key) = state.ui.pending_exit_hint() {
        return StatusBarView::ExitPrompt {
            key,
            text: crate::i18n::t!("status.exit_prompt", key = key.label()).to_string(),
        };
    }

    if let Some(status_line) = state.ui.display_settings.status_line.as_ref() {
        let padding = status_line.as_command().padding.max(0) as usize;
        let mut line = " ".repeat(padding);
        let custom = state.ui.status_line.last_success().unwrap_or("");
        line.push_str(custom);
        return StatusBarView::Custom { line };
    }

    StatusBarView::BuiltIn {
        lines: builtin::built_in_status_lines(state),
    }
}

/// Rows the status bar occupies for the given state. Used by the viewport to
/// reserve layout height before rendering. Cheap: avoids building spans.
/// `ExitPrompt` and a user-configured `Custom` status line stay single-row;
/// the built-in bar is one-to-three rows depending on populated content.
pub(crate) fn status_bar_height(state: &AppState) -> u16 {
    if state.ui.pending_exit_hint().is_some() {
        return 1;
    }
    if state.ui.display_settings.status_line.is_some() {
        return 1;
    }
    builtin::built_in_line_count(state)
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
