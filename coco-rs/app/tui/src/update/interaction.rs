//! Interaction precedence shell.
//!
//! Bottom-pane prompts and autocomplete have ordering rules that span more
//! than one surface. Modal-specific behavior lives in [`crate::modal_pane`].

use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::state::AppState;

/// Handle `Approve` for the current prompt/modal. The focused bottom-pane
/// prompt wins; modal surfaces are the fallback.
pub(super) async fn approve(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    if crate::bottom_pane::route_approve(state, command_tx).await {
        return;
    }
    crate::modal_pane::approve(state, command_tx).await;
}

/// Handle `Deny` for the current prompt/modal. The focused bottom-pane
/// prompt wins; modal surfaces are the fallback.
pub(super) async fn deny(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    if crate::bottom_pane::route_deny(state, command_tx).await {
        return;
    }
    crate::modal_pane::deny(state, command_tx).await;
}

/// Push `c` into the current prompt or filterable modal.
pub(super) fn filter(state: &mut AppState, c: char) {
    if crate::bottom_pane::route_filter(state, c) {
        return;
    }
    crate::modal_pane::filter(state, c);
}

/// Pop the last char from the current prompt or filterable modal.
pub(super) fn filter_backspace(state: &mut AppState) {
    if crate::bottom_pane::route_filter_backspace(state) {
        return;
    }
    crate::modal_pane::filter_backspace(state);
}

/// Move selection by `delta` in autocomplete, prompt, or modal state.
pub(super) fn nav(state: &mut AppState, delta: i32) {
    if !state.ui.has_blocking_interaction()
        && let Some(ref mut sug) = state.ui.completion.active
    {
        if sug.items.is_empty() {
            sug.selected = 0;
        } else {
            let new = sug.selected as i32 + delta;
            sug.selected = new.clamp(0, sug.items.len() as i32 - 1) as usize;
        }
        return;
    }
    if crate::bottom_pane::route_nav(state, delta) {
        return;
    }
    crate::modal_pane::nav(state, delta);
}

/// Confirm the currently selected item in the active prompt/modal.
pub(super) async fn confirm(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    if !state.ui.has_blocking_interaction() && state.ui.completion.active.is_some() {
        let _ = crate::completion::accept_suggestion(
            state,
            crate::completion::AcceptMode::AcceptSelected,
        );
        return;
    }

    if crate::modal_pane::route_confirm(state, command_tx).await {
        return;
    }

    let Some(prompt) = state.ui.take_prompt() else {
        return;
    };
    crate::bottom_pane::route_confirm(state, prompt, command_tx).await;
}

#[cfg(test)]
#[path = "interaction.test.rs"]
mod tests;
