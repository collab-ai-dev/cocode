//! `app:toggleTranscript` handler — open / close the transcript state.
//!
//! Two-way toggle:
//!
//! * not in transcript → open `ModalState::Transcript` with default state
//! * already in transcript → dismiss the modal (back to chat)
//!
//! coco-rs's transcript is a cell-level reader. It keeps expansion state in
//! the state only and does not implement a show-all path.

use crate::state::AppState;
use crate::state::ModalState;
use crate::state::transcript::TranscriptScrollPosition;
use crate::state::transcript::TranscriptState;

/// Open the transcript state if it isn't open; close it if it is.
pub(super) fn toggle(state: &mut AppState) {
    if matches!(state.ui.modal, Some(ModalState::Transcript(_))) {
        state.ui.dismiss_modal();
    } else {
        let anchor = crate::presentation::transcript::latest_expandable_cell_id(state);
        state
            .ui
            .show_modal(ModalState::Transcript(TranscriptState::new_with_anchor(
                anchor,
            )));
    }
}

pub(super) fn scroll_lines(state: &mut AppState, delta: i32) -> bool {
    let Some(ModalState::Transcript(state)) = state.ui.modal.as_mut() else {
        return false;
    };
    state.scroll.scroll_lines(delta);
    true
}

pub(super) fn page(state: &mut AppState, delta: i32) -> bool {
    let amount = transcript_page_rows(state);
    let Some(ModalState::Transcript(state)) = state.ui.modal.as_mut() else {
        return false;
    };
    let signed = amount.min(i32::MAX as usize) as i32;
    state
        .scroll
        .scroll_lines(if delta < 0 { -signed } else { signed });
    true
}

pub(super) fn jump_start(state: &mut AppState) -> bool {
    let Some(ModalState::Transcript(state)) = state.ui.modal.as_mut() else {
        return false;
    };
    state.scroll.jump_start();
    true
}

pub(super) fn jump_end(state: &mut AppState) -> bool {
    let Some(ModalState::Transcript(state)) = state.ui.modal.as_mut() else {
        return false;
    };
    state.scroll.jump_end();
    true
}

pub(super) fn select_expandable(state: &mut AppState, delta: i32) -> bool {
    let ids = crate::presentation::transcript::transcript_expandable_cell_ids(state);
    let Some(ModalState::Transcript(state)) = state.ui.modal.as_mut() else {
        return false;
    };
    if ids.is_empty() {
        state.selected_cell_id = None;
        return true;
    }

    let current = state
        .selected_cell_id
        .as_ref()
        .and_then(|id| ids.iter().position(|candidate| candidate == id));
    let next = match (current, delta.cmp(&0)) {
        (Some(index), std::cmp::Ordering::Less | std::cmp::Ordering::Greater) => {
            (index as i32 + delta).rem_euclid(ids.len() as i32) as usize
        }
        (Some(index), std::cmp::Ordering::Equal) => index,
        (None, std::cmp::Ordering::Less) => ids.len() - 1,
        (None, _) => 0,
    };
    state.selected_cell_id = Some(ids[next].clone());
    state.scroll = TranscriptScrollPosition::anchor(ids[next].clone());
    true
}

pub(super) fn toggle_selected_cell(state: &mut AppState) -> bool {
    let expandable = crate::presentation::transcript::transcript_expandable_cell_ids(state);
    let Some(ModalState::Transcript(state)) = state.ui.modal.as_mut() else {
        return false;
    };
    let Some(id) = state.selected_cell_id.clone() else {
        return true;
    };
    if !expandable.iter().any(|candidate| candidate == &id) {
        return true;
    }
    if !state.collapsed_cell_ids.insert(id.clone()) {
        state.collapsed_cell_ids.remove(&id);
    }
    state.scroll = TranscriptScrollPosition::anchor(id);
    true
}

fn transcript_page_rows(state: &AppState) -> usize {
    let size = state.ui.terminal_size;
    // Transcript uses the alt-screen with one border row on each side and a
    // two-line footer when space allows.
    usize::from(size.height.saturating_sub(4)).max(1)
}

#[cfg(test)]
#[path = "transcript.test.rs"]
mod tests;
