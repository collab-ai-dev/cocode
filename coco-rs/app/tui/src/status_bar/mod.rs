//! App-owned status bar model, custom statusLine runtime, and rendering.

mod builtin;
pub(crate) mod runtime;
mod widget;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StatusBarView {
    ExitPrompt { key: ExitKey, text: String },
    Custom { line: String },
    BuiltIn { lines: Vec<Vec<StatusSpan>> },
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
        line.push_str(state.ui.status_line.last_success().unwrap_or(""));
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
