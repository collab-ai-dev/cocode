//! `chat:stash` handler — single-slot push/pop input draft.
//!
//! One slot, three cases:
//!
//! * empty input + slot present → pop (restore stash to input + paste
//!   state, clear the slot)
//! * non-empty input → push (overwrite slot with current text + cursor
//!   + paste entries, clear input + paste state)
//! * empty input + empty slot → silent no-op
//!
//! `input.trim() === ''` tests emptiness, so whitespace-only input
//! behaves like empty.
//!
//! Stashes `(text, cursor, paste_entries)`. Pill labels embedded in `text`
//! (e.g. `[Pasted text #1]`) keep resolving after a pop because the
//! paste-manager entries are restored alongside the text.

use crate::state::AppState;
use crate::state::ui::StashedInput;

/// Push or pop the stash slot per the TS rules above.
pub(super) fn swap_input_draft(state: &mut AppState) {
    if state.ui.input.text().trim().is_empty() {
        // Empty input — pop the stash if there is one. Otherwise the
        // call is a silent no-op (matches TS implicit else).
        if let Some(prior) = state.ui.stashed_input.take() {
            state.ui.input.textarea.set_text(&prior.text);
            state.ui.input.textarea.set_cursor(prior.cursor_byte);
            state.ui.paste_manager.replace_entries(prior.paste_entries);
        }
    } else {
        // Non-empty input — push to the slot, overwriting any prior
        // stash (TS deliberately allows this; there is no swap or
        // stash list, just one slot). Paste entries move with the
        // text so pill labels stay resolvable.
        let cursor_byte = state.ui.input.textarea.cursor();
        let text = state.ui.input.textarea.take_text();
        state.ui.stashed_input = Some(StashedInput {
            text,
            cursor_byte,
            paste_entries: state.ui.paste_manager.take_entries(),
        });
    }
}

#[cfg(test)]
#[path = "stash.test.rs"]
mod tests;
