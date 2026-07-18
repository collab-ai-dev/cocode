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
use crate::state::transcript::TranscriptSearchRevision;
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

pub(super) fn search_start(state: &mut AppState) -> bool {
    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_mut() else {
        return false;
    };
    transcript.search.editing = true;
    let index_changed = ensure_search_index(state);
    if index_changed {
        refresh_search_matches(state, /*preserve_current*/ true);
    }
    true
}

pub(super) fn search_insert(state: &mut AppState, c: char) -> bool {
    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_mut() else {
        return false;
    };
    if !transcript.search.editing {
        return false;
    }
    transcript.search.query.push(c);
    ensure_search_index(state);
    refresh_search_matches(state, /*preserve_current*/ false);
    reveal_current_match(state);
    true
}

pub(super) fn search_backspace(state: &mut AppState) -> bool {
    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_mut() else {
        return false;
    };
    if !transcript.search.editing {
        return false;
    }
    transcript.search.query.pop();
    ensure_search_index(state);
    refresh_search_matches(state, /*preserve_current*/ false);
    reveal_current_match(state);
    true
}

pub(super) fn search_submit(state: &mut AppState) -> bool {
    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_mut() else {
        return false;
    };
    transcript.search.editing = false;
    ensure_search_index(state);
    refresh_search_matches(state, /*preserve_current*/ true);
    reveal_current_match(state);
    true
}

pub(super) fn search_dismiss(state: &mut AppState) -> bool {
    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_mut() else {
        return false;
    };
    transcript.search.editing = false;
    true
}

pub(super) fn search_navigate(state: &mut AppState, delta: i32) -> bool {
    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_ref() else {
        return false;
    };
    if transcript.search.query.is_empty() {
        return search_start(state);
    }
    if ensure_search_index(state) {
        refresh_search_matches(state, /*preserve_current*/ true);
    }
    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_mut() else {
        return false;
    };
    let len = transcript.search.matches.len();
    if len == 0 {
        transcript.search.cursor = None;
        return true;
    }
    transcript.search.cursor = Some(match transcript.search.cursor {
        Some(cursor) => (cursor as i32 + delta).rem_euclid(len as i32) as usize,
        None if delta < 0 => len - 1,
        None => 0,
    });
    reveal_current_match(state);
    true
}

fn search_revision(state: &AppState) -> TranscriptSearchRevision {
    TranscriptSearchRevision {
        transcript: state.session.transcript.revision(),
        stream: state
            .ui
            .streaming
            .as_ref()
            .map(|stream| stream.visible_generation),
        width: state.ui.terminal_size.width.saturating_sub(2).max(1),
        side_caches: crate::transcript::search_index::side_cache_revision(state),
    }
}

fn ensure_search_index(state: &mut AppState) -> bool {
    let revision = search_revision(state);
    let needs_rebuild = matches!(
        state.ui.modal.as_ref(),
        Some(ModalState::Transcript(transcript))
            if transcript.search.indexed_revision != Some(revision)
    );
    if !needs_rebuild {
        return false;
    }

    let (previous_entries, previous_revisions) = match state.ui.modal.as_mut() {
        Some(ModalState::Transcript(transcript)) => (
            std::mem::take(&mut transcript.search.entries),
            std::mem::take(&mut transcript.search.entry_revisions),
        ),
        _ => return false,
    };
    let build = crate::transcript::search_index::build_search_entries(
        state,
        revision.width,
        revision.stream,
        previous_entries,
        previous_revisions,
    );

    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_mut() else {
        return false;
    };
    transcript.search.entries = build.entries;
    transcript.search.entry_revisions = build.revisions;
    #[cfg(test)]
    {
        transcript.search.reused_entries_last_build = build.reused_entries;
    }
    transcript.search.indexed_revision = Some(revision);
    true
}

fn refresh_search_matches(state: &mut AppState, preserve_current: bool) {
    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_mut() else {
        return;
    };
    let current = preserve_current
        .then(|| transcript.search.current_match().cloned())
        .flatten();
    transcript.search.matches = crate::transcript::search::find_matches(
        &transcript.search.entries,
        &transcript.search.query,
    );
    transcript.search.cursor = current
        .and_then(|current| {
            transcript
                .search
                .matches
                .iter()
                .position(|candidate| candidate == &current)
        })
        .or_else(|| (!transcript.search.matches.is_empty()).then_some(0));
}

fn reveal_current_match(state: &mut AppState) {
    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_mut() else {
        return;
    };
    let Some(current) = transcript.search.current_match().cloned() else {
        return;
    };
    // Search indexes the full projection. If the user previously collapsed
    // this cell, revealing a hit must make the matched content visible.
    transcript.collapsed_cell_ids.remove(&current.cell_id);
    transcript.scroll = TranscriptScrollPosition::anchor_line(current.cell_id, current.row_offset);
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
