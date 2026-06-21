//! Background-tasks dialog: key handling for the full-screen modal opened
//! from the footer "N shells" pill (down-arrow at the bottom of the composer
//! parks focus there; Enter opens this dialog).
//!
//! The list/detail layers are rendered by
//! `presentation::picker_styled::background_tasks_lines`; this module owns only
//! navigation: ↑/↓ select (or scroll the detail), Enter drill-in / close,
//! `x` stop the focused task, ←/Esc back-or-close. Rows are derived live from
//! `SessionState::running_background_tasks`, so the modal state holds just the
//! cursor and which task (if any) is expanded.

use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::state::AppState;
use crate::state::BackgroundTasksState;
use crate::state::ModalState;

/// Outcome of [`intercept`]. Mirrors `agents_dialog::Handled`.
pub(super) enum Handled {
    Yes(bool),
    No,
}

/// Open the background-tasks dialog at the first row.
pub(super) fn open(state: &mut AppState) {
    state
        .ui
        .show_modal(ModalState::BackgroundTasks(BackgroundTasksState::default()));
}

pub(super) async fn intercept(
    state: &mut AppState,
    cmd: &TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> Handled {
    if !matches!(
        state.ui.modal.as_ref(),
        Some(ModalState::BackgroundTasks(_))
    ) {
        return Handled::No;
    }

    // Snapshot the live row ids before taking a mutable borrow of the modal.
    let row_ids: Vec<String> = state
        .session
        .running_background_tasks()
        .iter()
        .map(|t| t.task_id.clone())
        .collect();
    let row_count = row_ids.len();

    match cmd {
        // ↑/↓ move the list selection; in the detail view they are consumed
        // but inert (its output is a placeholder until live output lands).
        TuiCommand::CursorUp => {
            move_list_selection(state, -1, row_count);
            Handled::Yes(true)
        }
        TuiCommand::CursorDown => {
            move_list_selection(state, 1, row_count);
            Handled::Yes(true)
        }
        TuiCommand::SubmitInput => {
            toggle_detail(state, &row_ids);
            Handled::Yes(true)
        }
        TuiCommand::Cancel | TuiCommand::CursorLeft => {
            // From the detail layer, step back to the list; from the list,
            // close the dialog. Branches are exclusive so the immutable
            // probe and the mutable edit don't overlap.
            if in_detail(state) {
                if let Some(bt) = bt_mut(state) {
                    bt.detail = None;
                }
            } else {
                state.ui.dismiss_modal();
            }
            Handled::Yes(true)
        }
        TuiCommand::InsertChar(c) if c.eq_ignore_ascii_case(&'x') => {
            stop_focused(state, &row_ids, command_tx).await;
            Handled::Yes(true)
        }
        _ => Handled::No,
    }
}

fn bt_mut(state: &mut AppState) -> Option<&mut BackgroundTasksState> {
    match state.ui.modal.as_mut() {
        Some(ModalState::BackgroundTasks(bt)) => Some(bt),
        _ => None,
    }
}

fn in_detail(state: &AppState) -> bool {
    matches!(
        state.ui.modal.as_ref(),
        Some(ModalState::BackgroundTasks(bt)) if bt.detail.is_some()
    )
}

/// Move the list cursor by `delta`, clamped to `count`. No-op in the detail
/// view (its arrows are inert until live output is scrollable).
fn move_list_selection(state: &mut AppState, delta: i32, count: usize) {
    let Some(bt) = bt_mut(state) else { return };
    if bt.detail.is_some() {
        return;
    }
    if count == 0 {
        bt.selected = 0;
        return;
    }
    let max = count as i32 - 1;
    bt.selected = (bt.selected as i32 + delta).clamp(0, max) as usize;
}

/// Enter: from the list, drill into the focused task; from the detail layer,
/// close the dialog (matches TS Esc/Enter/Space "close").
fn toggle_detail(state: &mut AppState, row_ids: &[String]) {
    if in_detail(state) {
        state.ui.dismiss_modal();
        return;
    }
    if let Some(bt) = bt_mut(state) {
        let idx = bt.selected.min(row_ids.len().saturating_sub(1));
        if let Some(id) = row_ids.get(idx) {
            bt.detail = Some(id.clone());
        }
    }
}

/// `x`: cancel the focused task via `CancelSubagent`, which fires the task's
/// cancellation token (works for shells and agents alike). The engine's
/// `TaskCompleted` event then folds the row out of the list.
async fn stop_focused(
    state: &mut AppState,
    row_ids: &[String],
    command_tx: &mpsc::Sender<UserCommand>,
) {
    let task_id = match state.ui.modal.as_ref() {
        Some(ModalState::BackgroundTasks(bt)) => bt.detail.clone().or_else(|| {
            row_ids
                .get(bt.selected.min(row_ids.len().saturating_sub(1)))
                .cloned()
        }),
        _ => None,
    };
    if let Some(task_id) = task_id {
        let _ = command_tx
            .send(UserCommand::CancelSubagent { task_id })
            .await;
    }
}
