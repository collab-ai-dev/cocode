//! `chat:stash` handler — single-slot push/pop input draft.
//!
//! One slot, three cases:
//!
//! * empty input + slot present → pop the complete composer snapshot
//! * non-empty input → push the complete composer snapshot
//! * empty input + empty slot → silent no-op
//!
//! `input.trim() === ''` tests emptiness, so whitespace-only input
//! behaves like empty.
//!
//! Text, cursor, elements, and payloads move through one typed snapshot.

use crate::state::AppState;
use crate::state::ui::StashedInput;

/// Push or pop the stash slot per the TS rules above.
pub(super) fn swap_input_draft(state: &mut AppState) {
    if state.ui.input.text().trim().is_empty() {
        // Empty input — pop the stash if there is one. Otherwise the
        // call is a silent no-op (matches TS implicit else).
        if let Some(prior) = state.ui.stashed_input.take() {
            state.ui.input.restore_composer(prior.composer);
        }
    } else {
        // Non-empty input — push to the slot, overwriting any prior
        // stash (TS deliberately allows this; there is no swap or
        // stash list, just one slot). Paste entries move with the
        // text so pill labels stay resolvable.
        state.ui.stashed_input = Some(StashedInput {
            composer: state.ui.input.take_composer(),
        });
    }
}

#[cfg(test)]
#[path = "stash.test.rs"]
mod tests;
