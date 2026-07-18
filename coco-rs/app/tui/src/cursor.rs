//! Cursor decision — single point of truth.
//!
//! The active surface renderer returns a [`FrameLayout`]; this module turns
//! that plus `AppState` into an optional [`CursorClaim`]. `SurfaceTerminal`
//! applies the claim after drawing the retained viewport. No widget calls
//! `Frame::set_cursor_position` directly.
//!
//! Why post-draw instead of `Frame::set_cursor_position` inside the
//! closure: ratatui 0.30's `Frame` exposes no `set_cursor_style`, so the
//! cursor shape (bar / block / underline) can only be controlled via raw
//! `crossterm::cursor::SetCursorStyle` queued to stdout. Doing the whole
//! pin (style + position + show/hide) post-draw keeps the policy in one
//! place and makes the focus-gained / suspend-resume re-pin path
//! identical to the normal path.

use crossterm::cursor::SetCursorStyle;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use coco_tui_ui::engine::CursorClaim;
use coco_tui_ui::style::UiStyles;

use crate::FrameLayout;
use crate::presentation::request::project_question;
use crate::state::AppState;
use crate::state::FocusTarget;
use crate::state::PanePromptState;
use crate::state::QuestionFocusTarget;
use crate::widgets::InputRenderModel;

/// Decide where (and whether) the cursor goes for the next frame.
///
/// Single decision point: the input widget is the only base cursor source
/// today. Modals hide that base cursor unless they explicitly mirror
/// input text, as the command palette does. Returning `None` tells
/// `Tui::draw` to hide the cursor explicitly — see module docs for why hide
/// alone isn't enough on iTerm2 / macOS Terminal.
pub fn compute_cursor(state: &AppState, layout: FrameLayout) -> Option<CursorClaim> {
    // An active in-modal text input (e.g. the `/model` filter or a `/provider`
    // wizard field) pins the cursor at its caret. This must precede the
    // focus / blocking-interaction early-returns below: a modal is a blocking
    // interaction with `focus != Input`, so without this a CJK IME would
    // anchor its candidate window to the top-left corner instead of the field.
    if let Some(position) = layout.modal_text_cursor {
        return Some(CursorClaim {
            position,
            style: SetCursorStyle::DefaultUserShape,
        });
    }
    if let Some(claim) = compute_question_cursor(state, layout.question_prompt) {
        return Some(claim);
    }
    if state.ui.focus != FocusTarget::Input {
        return None;
    }
    if state.ui.has_blocking_interaction() {
        return None;
    }
    if layout.input.width == 0 || layout.input.height == 0 {
        return None;
    }
    let (x, y) = compute_input_xy(state, layout.input);
    Some(CursorClaim {
        position: Position { x, y },
        style: vim_cursor_style(&state.ui.input.vim),
    })
}

/// Cursor shape reflects vim mode so the user has an in-band tell of which mode
/// they are in — the universal block-vs-bar convention. Only active when vim
/// editing is enabled; otherwise the terminal's default shape is preserved.
fn vim_cursor_style(vim: &crate::vim::VimRuntime) -> SetCursorStyle {
    if !vim.enabled {
        return SetCursorStyle::DefaultUserShape;
    }
    if vim.is_normal() {
        SetCursorStyle::SteadyBlock
    } else {
        SetCursorStyle::SteadyBar
    }
}

fn compute_question_cursor(state: &AppState, area: Rect) -> Option<CursorClaim> {
    let Some(PanePromptState::Question(q)) = state.ui.interaction.active_prompt.as_ref() else {
        return None;
    };
    if q.focus_target != QuestionFocusTarget::OtherInput {
        return None;
    }
    if area.width == 0 || area.height == 0 {
        return None;
    }
    let styles = UiStyles::new(&state.ui.theme);
    let view = project_question(q);
    let position = view.input_cursor_position(area, styles)?;
    Some(CursorClaim {
        position,
        style: SetCursorStyle::DefaultUserShape,
    })
}

/// Compute the cursor's absolute terminal coordinates inside the input
/// widget's area. Empty input is intentionally NOT special-cased:
/// returning a real position even for an empty buffer is what fixes the
/// "cursor floats to the status bar on focus regain" bug — the cursor
/// always has a defined home.
fn compute_input_xy(state: &AppState, area: Rect) -> (u16, u16) {
    let is_streaming = state.is_streaming();
    // Built at the composer's real width so the cursor's visual row matches the
    // rows the renderer painted — the model wraps, so a width mismatch here
    // would put the cursor on the wrong line.
    let model = InputRenderModel::build(
        &state.ui.input,
        is_streaming,
        state.session.prompt_suggestions.last().map(String::as_str),
        state.session.queued_commands.iter().any(|q| q.editable),
        None,
        area.width,
    );

    let indicator_width: u16 = crate::widgets::INPUT_GUTTER_WIDTH;

    // Display column (CJK-aware). The command palette mirrors a `/filter`; every
    // other state uses the per-line column computed once in build().
    let raw_cursor: i32 = match model.command_palette_filter.as_deref() {
        Some(filter) => 1 + UnicodeWidthStr::width(filter) as i32,
        None => model.cursor_col as i32,
    };

    // Cursor row, offset by the same scroll the renderer uses so the two agree.
    let content_rows = area.height.saturating_sub(2).max(1) as usize;
    let total_rows = model.rows.len().max(1);
    let scroll = crate::widgets::scroll_offset(model.cursor_row, total_rows, content_rows);
    let row_on_screen = model.cursor_row.saturating_sub(scroll) as u16;

    let max_cursor = area.width.saturating_sub(indicator_width + 1) as i32;
    let cursor_x = area.x + indicator_width + raw_cursor.min(max_cursor) as u16;
    let cursor_y = area.y + 1 + row_on_screen;
    (cursor_x, cursor_y)
}

#[cfg(test)]
#[path = "cursor.test.rs"]
mod tests;
