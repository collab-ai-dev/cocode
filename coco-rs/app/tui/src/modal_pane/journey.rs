//! Key dispatch for the `/journey` learning-timeline overlay.
//!
//! Modes:
//!   - **list** — `j`/`k` (or ↑/↓) select a node, `Enter` opens detail, `e`
//!     edits (opens the backing file), `d` retires/restores a skill (immediate)
//!     or opens the memory-delete confirm, `Esc` closes.
//!   - **detail** — `Enter`/`Esc` returns to the list.
//!   - **delete-memory confirm** — ←/→ (or y/n) toggle, `Enter` commits, `Esc`
//!     cancels. Only memory deletion (irreversible) needs a confirm.
//!
//! Uses a dedicated Global-only keybinding context (the PermissionsEditor
//! precedent) so `j`/`k`/`e`/`d` reach this interceptor instead of being eaten
//! by the generic picker filter.

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::state::AppState;
use crate::state::JourneyMode;
use crate::state::JourneyState;
use crate::state::ModalState;
use coco_types::{
    AgentSkillLifecycleWire, JourneyAction, JourneyNodeBodyWire, JourneyNodeId, JourneyNodeWire,
};
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;

/// Keys for the `/journey` overlay. The overlay has no text input, so keys map
/// to nav/confirm commands directly.
pub(crate) fn map_key(key: KeyEvent) -> Option<TuiCommand> {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(TuiCommand::CursorDown),
        KeyCode::Char('k') | KeyCode::Up => Some(TuiCommand::CursorUp),
        KeyCode::Char('h') | KeyCode::Left => Some(TuiCommand::CursorLeft),
        KeyCode::Char('l') | KeyCode::Right => Some(TuiCommand::CursorRight),
        KeyCode::Enter => Some(TuiCommand::SubmitInput),
        KeyCode::Esc => Some(TuiCommand::Cancel),
        KeyCode::Char(c @ ('e' | 'd' | 'y' | 'n')) => Some(TuiCommand::InsertChar(c)),
        _ => None,
    }
}

/// Outcome of [`intercept`]. `Yes(true)` ⇒ state changed (redraw); `Yes(false)`
/// ⇒ key swallowed without a visible effect.
pub(crate) enum Handled {
    Yes(bool),
    No,
}

pub(crate) async fn intercept(
    state: &mut AppState,
    cmd: &TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> Handled {
    let mode = match state.ui.modal.as_ref() {
        Some(ModalState::Journey(j)) => j.mode.clone(),
        _ => return Handled::No,
    };
    match mode {
        JourneyMode::DeleteMemoryConfirm { .. } => intercept_confirm(state, cmd, command_tx).await,
        JourneyMode::List | JourneyMode::Detail => {
            intercept_list_detail(state, cmd, command_tx).await
        }
    }
}

fn journey_mut(state: &mut AppState) -> Option<&mut JourneyState> {
    match state.ui.modal.as_mut() {
        Some(ModalState::Journey(j)) => Some(j),
        _ => None,
    }
}

async fn intercept_list_detail(
    state: &mut AppState,
    cmd: &TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> Handled {
    match cmd {
        TuiCommand::CursorDown => Handled::Yes(nav(state, 1)),
        TuiCommand::CursorUp => Handled::Yes(nav(state, -1)),
        TuiCommand::SubmitInput => Handled::Yes(toggle_detail(state)),
        TuiCommand::Cancel => {
            if back_to_list(state) {
                Handled::Yes(true)
            } else {
                state.ui.dismiss_modal();
                Handled::Yes(true)
            }
        }
        TuiCommand::InsertChar('e') => Handled::Yes(edit_selected(state, command_tx).await),
        TuiCommand::InsertChar('d') => {
            Handled::Yes(delete_or_retire_selected(state, command_tx).await)
        }
        _ => Handled::Yes(false),
    }
}

/// Move the selection (list mode only — detail ignores nav).
fn nav(state: &mut AppState, delta: i32) -> bool {
    let Some(j) = journey_mut(state) else {
        return false;
    };
    if j.mode != JourneyMode::List {
        return false;
    }
    j.nav(delta);
    true
}

/// Toggle the detail sub-mode for the current selection.
fn toggle_detail(state: &mut AppState) -> bool {
    let Some(j) = journey_mut(state) else {
        return false;
    };
    j.mode = match j.mode {
        JourneyMode::Detail => JourneyMode::List,
        _ if j.selected_node().is_some() => JourneyMode::Detail,
        _ => return false,
    };
    true
}

/// From Detail, return to List (returns false when already in List so the
/// caller closes the overlay instead).
fn back_to_list(state: &mut AppState) -> bool {
    let Some(j) = journey_mut(state) else {
        return false;
    };
    if j.mode == JourneyMode::Detail {
        j.mode = JourneyMode::List;
        true
    } else {
        false
    }
}

/// `e` — open the selected node's backing file in the external editor. The CLI
/// resolves the path (memory filenames against the memdir) and refreshes on
/// return.
async fn edit_selected(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) -> bool {
    let Some(node) = journey_selected(state) else {
        return false;
    };
    let action = JourneyAction::OpenInEditor {
        id: node_id_of(node),
    };
    let _ = command_tx
        .send(UserCommand::ApplyJourneyAction { action })
        .await;
    false
}

/// `d` — retire/restore an agent skill immediately (a reversible `disabled`
/// flip), or open the memory-delete confirm (irreversible). User skills are
/// left alone (curator write-only philosophy).
async fn delete_or_retire_selected(
    state: &mut AppState,
    command_tx: &mpsc::Sender<UserCommand>,
) -> bool {
    let Some(node) = journey_selected(state) else {
        return false;
    };
    match &node.body {
        JourneyNodeBodyWire::AgentSkill {
            path, lifecycle, ..
        } => {
            let path = PathBuf::from(path);
            let action = match lifecycle {
                AgentSkillLifecycleWire::Retired => JourneyAction::RestoreSkill { path },
                _ => JourneyAction::RetireSkill { path },
            };
            let _ = command_tx
                .send(UserCommand::ApplyJourneyAction { action })
                .await;
            false
        }
        JourneyNodeBodyWire::Memory { .. } => {
            if let Some(j) = journey_mut(state) {
                j.mode = JourneyMode::DeleteMemoryConfirm {
                    yes_selected: false,
                };
            }
            true
        }
        JourneyNodeBodyWire::UserSkill { .. } => false,
    }
}

/// Delete-memory confirm sub-mode: ←/→ (or y/n) toggle, Enter commits, Esc
/// cancels back to the list.
async fn intercept_confirm(
    state: &mut AppState,
    cmd: &TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> Handled {
    match cmd {
        TuiCommand::CursorLeft | TuiCommand::CursorRight => {
            if let Some(JourneyMode::DeleteMemoryConfirm { yes_selected }) =
                journey_mut(state).map(|j| &mut j.mode)
            {
                *yes_selected = !*yes_selected;
            }
            Handled::Yes(true)
        }
        TuiCommand::InsertChar('y') => {
            set_confirm(state, true);
            Handled::Yes(true)
        }
        TuiCommand::InsertChar('n') => {
            set_confirm(state, false);
            Handled::Yes(true)
        }
        TuiCommand::SubmitInput => {
            let (confirmed, filename) = confirm_target(state);
            if confirmed && let Some(filename) = filename {
                let _ = command_tx
                    .send(UserCommand::ApplyJourneyAction {
                        action: JourneyAction::DeleteMemory { filename },
                    })
                    .await;
            }
            if let Some(j) = journey_mut(state) {
                j.mode = JourneyMode::List;
            }
            Handled::Yes(true)
        }
        TuiCommand::Cancel => {
            if let Some(j) = journey_mut(state) {
                j.mode = JourneyMode::List;
            }
            Handled::Yes(true)
        }
        _ => Handled::Yes(false),
    }
}

fn set_confirm(state: &mut AppState, value: bool) {
    if let Some(JourneyMode::DeleteMemoryConfirm { yes_selected }) =
        journey_mut(state).map(|j| &mut j.mode)
    {
        *yes_selected = value;
    }
}

/// `(confirmed, memdir-relative filename)` for the pending delete confirm.
fn confirm_target(state: &AppState) -> (bool, Option<String>) {
    let Some(ModalState::Journey(j)) = state.ui.modal.as_ref() else {
        return (false, None);
    };
    let confirmed = matches!(
        j.mode,
        JourneyMode::DeleteMemoryConfirm { yes_selected: true }
    );
    let filename = j.selected_node().and_then(|n| match &n.body {
        JourneyNodeBodyWire::Memory { filename } => Some(filename.clone()),
        _ => None,
    });
    (confirmed, filename)
}

fn journey_selected(state: &AppState) -> Option<&JourneyNodeWire> {
    match state.ui.modal.as_ref() {
        Some(ModalState::Journey(j)) => j.selected_node(),
        _ => None,
    }
}

fn node_id_of(node: &JourneyNodeWire) -> JourneyNodeId {
    match &node.body {
        JourneyNodeBodyWire::AgentSkill { path, .. }
        | JourneyNodeBodyWire::UserSkill { path, .. } => JourneyNodeId::Skill {
            path: PathBuf::from(path),
        },
        JourneyNodeBodyWire::Memory { filename } => JourneyNodeId::Memory {
            filename: filename.clone(),
        },
    }
}

#[cfg(test)]
#[path = "journey.test.rs"]
mod tests;
