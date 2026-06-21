//! The native-scrollback paint engine: synchronized-update (BSU/ESU) framing,
//! cell-diff drawing, history insertion/reflow, viewport seating, and
//! terminal-capability detection. Domain-free — the shell projects `AppState`
//! into `Line`s and a `CursorClaim` and drives this engine to paint them.

pub mod compatibility;
pub mod history_insert;
pub mod history_reflow;
pub mod seat;
pub mod terminal;

use crossterm::cursor::SetCursorStyle;
use ratatui::layout::Position;

/// Where (and how) the cursor is placed for a frame.
///
/// The shell computes this via its own `compute_cursor(&AppState)` (the single
/// decision point) and hands it to [`terminal::SurfaceTerminal`] to apply after
/// the frame is painted.
#[derive(Debug, Clone, Copy)]
pub struct CursorClaim {
    pub position: Position,
    pub style: SetCursorStyle,
}
