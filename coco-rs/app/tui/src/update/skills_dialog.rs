//! Key dispatch for the editable `/skills` dialog.
//!
//! Two modes:
//!
//! - **Select** (default) — `Space` cycles the focused row's
//!   override, `Enter` saves + closes, `Esc` cancels + closes,
//!   `↑`/`↓` move selection, `/` enters filter mode, `t` toggles
//!   sort, any other printable char drops into filter mode and
//!   appends.
//! - **Filter** — every printable char appends to the query (the
//!   literal `/` is stripped on input), `Backspace` pops,
//!   `Enter`/`↓` exit filter focus, `Esc` clears query.

use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::state::AppState;
use crate::state::ModalState;

/// Outcome of [`intercept`] — distinguishes "I handled this, don't
/// run the main dispatch" from "let the main dispatch keep going."
pub(super) enum Handled {
    /// Skip the main `match cmd` block; redraw with the returned bool.
    Yes(bool),
    /// Not a skills-dialog key; fall through to the main dispatch.
    No,
}

/// Intercept `cmd` when the [`ModalState::SkillsDialog`] modal is
/// active. Returns [`Handled::Yes`] for keys the dialog consumes,
/// [`Handled::No`] to let the caller continue with its normal
/// dispatch.
pub(super) async fn intercept(
    state: &mut AppState,
    cmd: &TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> Handled {
    let Some(ModalState::SkillsDialog(_)) = state.ui.modal.as_ref() else {
        return Handled::No;
    };
    match cmd {
        TuiCommand::InsertChar(c) => Handled::Yes(handle_char(state, *c)),
        TuiCommand::InsertNewline => Handled::Yes(handle_char(state, '\n')),
        TuiCommand::DeleteBackward => Handled::Yes(handle_backspace(state)),
        TuiCommand::SubmitInput => {
            handle_submit(state, command_tx).await;
            Handled::Yes(true)
        }
        TuiCommand::Cancel => {
            handle_cancel(state);
            Handled::Yes(true)
        }
        TuiCommand::CursorUp => Handled::Yes(handle_arrow_up(state)),
        TuiCommand::CursorDown => Handled::Yes(handle_arrow_down(state)),
        _ => Handled::No,
    }
}

fn dialog_mut(state: &mut AppState) -> &mut crate::state::SkillsDialogState {
    match state.ui.modal.as_mut() {
        Some(ModalState::SkillsDialog(d)) => d,
        _ => unreachable!("intercept guarded on ModalState::SkillsDialog"),
    }
}

fn handle_char(state: &mut AppState, c: char) -> bool {
    let dialog = dialog_mut(state);
    if dialog.filter_focused {
        // Newlines never make sense in the filter — swallow them
        // silently so the normal `InsertNewline` path can't insert
        // into the prompt textarea when the modal is open.
        if c == '\n' {
            return false;
        }
        dialog.apply_filter_char(c);
        return true;
    }
    // Select mode: special keys before falling through to filter.
    match c {
        ' ' => {
            dialog.cycle_focused();
            true
        }
        '/' => {
            dialog.filter_focused = true;
            true
        }
        't' => {
            dialog.toggle_sort();
            true
        }
        '\n' => false,
        _ => {
            // Any other printable drops into filter mode and pushes the char.
            dialog.filter_focused = true;
            dialog.apply_filter_char(c);
            true
        }
    }
}

fn handle_backspace(state: &mut AppState) -> bool {
    let dialog = dialog_mut(state);
    if dialog.filter_focused {
        return dialog.backspace_filter();
    }
    // Select mode: swallow Backspace when no query exists.
    // If the query is non-empty, refocus filter and pop one char.
    if !dialog.filter_query.is_empty() {
        dialog.filter_focused = true;
        return dialog.backspace_filter();
    }
    false
}

async fn handle_submit(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    let dialog = dialog_mut(state);
    if dialog.filter_focused {
        // Enter inside filter mode exits filter focus but keeps the query active.
        dialog.filter_focused = false;
        return;
    }
    let diff = dialog.compute_save_diff();
    let has_disk_changes = diff.has_disk_changes();
    let total_edits = diff.total_edits;
    state.ui.dismiss_modal();
    if !has_disk_changes {
        // Pure no-op save — every toggled row landed back at its
        // baseline. Render the "No changes" toast locally and skip
        // the round-trip; the CLI handler would do exactly this but
        // we save a settings-file touch + republish cycle.
        let text = crate::i18n::t!("dialog.skills_save_no_changes").to_string();
        state.ui.add_toast(crate::state::ui::Toast::info(text));
        return;
    }
    // Stash the count locally so the `SkillOverridesSaved` event
    // handler can render the localized toast without `total_edits`
    // round-tripping through the CLI bridge. CLI only reports
    // success / typed failure.
    state.ui.pending_skills_save_edits = Some(total_edits);
    let patch = diff.to_settings_patch();
    let _ = command_tx
        .send(UserCommand::WriteSkillOverrides { patch })
        .await;
}

fn handle_cancel(state: &mut AppState) {
    let dialog = dialog_mut(state);
    if dialog.filter_focused || !dialog.filter_query.is_empty() {
        // Esc inside filter mode clears the query + exits focus.
        // The dialog stays open; the user can keep navigating.
        dialog.clear_filter();
        return;
    }
    // No filter active — Esc cancels the dialog entirely; any
    // pending in-memory edits are discarded.
    state.ui.dismiss_modal();
}

fn handle_arrow_up(state: &mut AppState) -> bool {
    let dialog = dialog_mut(state);
    dialog.move_up();
    true
}

fn handle_arrow_down(state: &mut AppState) -> bool {
    let dialog = dialog_mut(state);
    if dialog.filter_focused {
        // ↓ from filter exits focus + keeps the query active.
        // The list cursor remains on the first filtered row.
        dialog.filter_focused = false;
        return true;
    }
    dialog.move_down();
    true
}

#[cfg(test)]
#[path = "skills_dialog.test.rs"]
mod tests;
