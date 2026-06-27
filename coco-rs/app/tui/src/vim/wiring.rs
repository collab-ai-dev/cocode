//! Bridge between key dispatch and the vim state machine.
//!
//! The state machine in `transitions.rs` / `operators.rs` / `motions.rs`
//! operates directly on `TextArea`'s byte-offset cursor model, so this
//! module is just a thin routing layer:
//!
//! - `dispatch_vim_key` — call into the state machine while in Normal mode.
//! - `apply_action` — translate the returned `VimAction` (mode transitions
//!   like `i`/`a`/`A`/`o`/`O`) into TextArea cursor adjustments.
//! - `handle_insert_escape` — Esc in Insert mode returns to Normal and
//!   walks the cursor back one grapheme (vim convention).
//!
//! Mirrors codex-rs's pattern at
//! `codex-rs/tui/src/bottom_pane/textarea.rs:518-530` and 646+
//! (`handle_vim_input` → `handle_vim_normal`).

use super::VimRuntime;
use super::VimState;
use super::transitions;
use super::transitions::VimAction;
use coco_tui_ui::widgets::TextArea;

/// Dispatch a printable key through the vim state machine while in Normal
/// mode. In Insert mode the call is a no-op (`VimAction::Unhandled`) — the
/// caller falls through to `textarea.insert_str` so typing works normally.
///
/// Captures a pre-dispatch snapshot and commits it to the undo stack
/// ONLY when the dispatched key actually mutated the buffer (and it
/// wasn't `u` itself, which would otherwise turn undo into a redo-flip).
pub fn dispatch_vim_key(ch: char, textarea: &mut TextArea, vim: &mut VimRuntime) -> VimAction {
    let VimState::Normal { command } = &mut vim.state else {
        return VimAction::Unhandled;
    };
    let snap = textarea.snapshot();
    let action = transitions::process_normal_key(ch, textarea, command, &mut vim.persistent);
    // `u` consumes the undo stack itself; don't push a redo-target snapshot
    // (we have no redo stack and this would just oscillate).
    if ch != 'u' && textarea.text() != snap.text() {
        textarea.commit_undo(snap);
    }
    action
}

/// Apply a `VimAction` returned by `dispatch_vim_key`. Handles the mode
/// transition variants (`EnterInsert*`, `OpenAbove`/`OpenBelow`) by
/// mutating `vim.state` and the TextArea cursor.
///
/// Returns `true` if the action was `Submit` — the caller should treat
/// the key as a fully-consumed submit gesture.
pub fn apply_action(action: VimAction, textarea: &mut TextArea, vim: &mut VimRuntime) -> bool {
    match action {
        VimAction::Handled | VimAction::Unhandled => false,
        VimAction::EnterInsert => {
            vim.state.enter_insert();
            false
        }
        VimAction::EnterInsertAfter => {
            // `a` — append after current grapheme.
            textarea.move_cursor_right();
            vim.state.enter_insert();
            false
        }
        VimAction::EnterInsertEnd => {
            // `A` — jump to end of current logical line.
            let eol = textarea.end_of_current_line();
            textarea.set_cursor(eol);
            vim.state.enter_insert();
            false
        }
        VimAction::EnterInsertHome => {
            // `I` — jump to beginning of current logical line.
            let bol = textarea.beginning_of_current_line();
            textarea.set_cursor(bol);
            vim.state.enter_insert();
            false
        }
        VimAction::OpenBelow => {
            // `o` — open new line below the current.
            let eol = textarea.end_of_current_line();
            textarea.set_cursor(eol);
            textarea.insert_str("\n");
            vim.state.enter_insert();
            false
        }
        VimAction::OpenAbove => {
            // `O` — open new line above the current. Insert `\n` at BOL,
            // then step back onto the freshly created blank line.
            let bol = textarea.beginning_of_current_line();
            textarea.set_cursor(bol);
            textarea.insert_str("\n");
            textarea.move_cursor_left();
            vim.state.enter_insert();
            false
        }
        VimAction::Submit => true,
    }
}

/// Handle Esc in vim Insert mode: switch to Normal mode and walk the
/// cursor back one grapheme (vim convention — the cursor lands on the
/// last typed character, not past it). Mirrors codex-rs textarea.rs:654-660.
///
/// Returns `true` if Esc was consumed (i.e. we were in vim insert), so
/// the caller can avoid running its own Cancel logic.
pub fn handle_insert_escape(textarea: &mut TextArea, vim: &mut VimRuntime) -> bool {
    if !vim.is_insert() {
        return false;
    }
    let bol = textarea.beginning_of_current_line();
    if textarea.cursor() > bol {
        textarea.move_cursor_left();
    }
    vim.state.enter_normal();
    true
}

#[cfg(test)]
#[path = "wiring.test.rs"]
mod tests;
