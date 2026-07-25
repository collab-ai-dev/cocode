//! Terminal-specific row caps for history replay and resize reflow.
//!
//! coco rebuilds the terminal's own scrollback rather than an internal virtual
//! transcript, so the useful ceiling is whatever the terminal retains. The
//! `Auto` table mirrors each terminal's documented scrollback default; replaying
//! past it costs render time and native-insert bandwidth for rows the user can
//! never scroll back to. VS Code is the case that motivated this: its integrated
//! terminal keeps 1000 lines by default, so the previous flat 9000-row cap did
//! ~9× the work on every width change.
//!
//! Terminals coco cannot identify fall back to [`FALLBACK_MAX_ROWS`], which is
//! the historical flat cap — an unknown terminal is not evidence of a *small*
//! scrollback.

use coco_config::settings::ReflowMaxRows;
use coco_tui_ui::terminal_detect::TerminalName;

/// Row cap for terminals whose scrollback default coco does not know.
pub(crate) const FALLBACK_MAX_ROWS: usize = 9_000;

const VSCODE_MAX_ROWS: usize = 1_000;
const WINDOWS_TERMINAL_MAX_ROWS: usize = 9_001;
const WEZTERM_MAX_ROWS: usize = 3_500;
const ALACRITTY_MAX_ROWS: usize = 10_000;

/// The cap for a terminal with no addressable scrollback. Small on purpose:
/// there is nothing to scroll back through.
const DUMB_MAX_ROWS: usize = 1_000;

/// Maximum transcript rows rebuilt into native scrollback on replay or width
/// change.
///
/// A newtype rather than a bare `usize` so a derived `Default` can never mean
/// "replay zero rows" — the safe default is the historical flat cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MaxReflowRows(usize);

impl MaxReflowRows {
    /// Rebuild the whole transcript however long it is.
    pub const UNLIMITED: Self = Self(usize::MAX);

    pub fn get(self) -> usize {
        self.0
    }
}

impl Default for MaxReflowRows {
    fn default() -> Self {
        Self(FALLBACK_MAX_ROWS)
    }
}

/// Resolve the configured row cap against the detected terminal.
pub(crate) fn resolve_max_reflow_rows(
    setting: ReflowMaxRows,
    terminal: TerminalName,
) -> MaxReflowRows {
    match setting {
        ReflowMaxRows::Auto => MaxReflowRows(auto_max_reflow_rows(terminal)),
        ReflowMaxRows::Unlimited => MaxReflowRows::UNLIMITED,
        // A configured 0 would rebuild an empty transcript, which reads as a
        // broken terminal rather than as a setting; treat it as "no cap".
        ReflowMaxRows::Rows(0) => MaxReflowRows::UNLIMITED,
        ReflowMaxRows::Rows(rows) => MaxReflowRows(rows),
    }
}

fn auto_max_reflow_rows(terminal: TerminalName) -> usize {
    match terminal {
        TerminalName::VsCode => VSCODE_MAX_ROWS,
        TerminalName::WindowsTerminal => WINDOWS_TERMINAL_MAX_ROWS,
        TerminalName::WezTerm => WEZTERM_MAX_ROWS,
        TerminalName::Alacritty => ALACRITTY_MAX_ROWS,
        TerminalName::Dumb => DUMB_MAX_ROWS,
        TerminalName::AppleTerminal
        | TerminalName::Ghostty
        | TerminalName::GnomeTerminal
        | TerminalName::Hyper
        | TerminalName::Iterm2
        | TerminalName::Kitty
        | TerminalName::Konsole
        | TerminalName::Unknown
        | TerminalName::Vte
        | TerminalName::Warp => FALLBACK_MAX_ROWS,
    }
}

#[cfg(test)]
#[path = "reflow_cap.test.rs"]
mod tests;
