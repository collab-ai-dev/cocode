//! Layout facts produced by a TUI frame draw.

use ratatui::layout::Rect;

/// Layout slots produced by the active surface renderer.
///
/// Today only the cursor decision reads this: it needs the bordered input
/// rect to compute the cursor position after the frame is drawn.
#[derive(Debug, Default, Clone, Copy)]
pub struct FrameLayout {
    /// Bordered input widget rect.
    ///
    /// `Rect::default()` when rendering did not reach the input, such as when
    /// a full-screen state owns the frame.
    pub input: Rect,
    /// AskUserQuestion prompt rect when that prompt owns the interaction area.
    ///
    /// `Rect::default()` when no question prompt is rendered.
    pub question_prompt: Rect,
}
