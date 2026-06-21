//! Key dispatch for the `/plugin` dialog.

use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::SlashCommandName;

pub(super) enum Handled {
    Yes(bool),
    No,
}

pub(super) async fn intercept(
    state: &mut AppState,
    cmd: &TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> Handled {
    let Some(ModalState::PluginDialog(_)) = state.ui.modal.as_ref() else {
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
        TuiCommand::SettingsNextTab => {
            dialog_mut(state).cycle_tab_next();
            Handled::Yes(true)
        }
        TuiCommand::SettingsPrevTab => {
            dialog_mut(state).cycle_tab_prev();
            Handled::Yes(true)
        }
        _ => Handled::No,
    }
}

fn dialog_mut(state: &mut AppState) -> &mut crate::state::PluginDialogState {
    match state.ui.modal.as_mut() {
        Some(ModalState::PluginDialog(d)) => d,
        _ => unreachable!("intercept guarded on ModalState::PluginDialog"),
    }
}

fn handle_char(state: &mut AppState, c: char) -> bool {
    let dialog = dialog_mut(state);
    if dialog.filter_focused {
        if c == '\n' {
            return false;
        }
        dialog.apply_filter_char(c);
        return true;
    }
    match c {
        '/' => {
            dialog.filter_focused = true;
            true
        }
        '\n' => false,
        _ => {
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
    if !dialog.filter_query.is_empty() {
        dialog.filter_focused = true;
        return dialog.backspace_filter();
    }
    false
}

async fn handle_submit(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    let action = dialog_mut(state).focused_action();
    let Some(action) = action else {
        return;
    };
    let Ok(name) = SlashCommandName::new("plugin") else {
        return;
    };
    let _ = command_tx
        .send(UserCommand::ExecuteSlashCommand {
            name,
            args: action.plugin_args,
        })
        .await;
}

fn handle_cancel(state: &mut AppState) {
    let dialog = dialog_mut(state);
    if dialog.filter_focused || !dialog.filter_query.is_empty() {
        dialog.clear_filter();
        return;
    }
    state.ui.dismiss_modal();
}

fn handle_arrow_up(state: &mut AppState) -> bool {
    dialog_mut(state).move_up();
    true
}

fn handle_arrow_down(state: &mut AppState) -> bool {
    let dialog = dialog_mut(state);
    if dialog.filter_focused {
        dialog.filter_focused = false;
        return true;
    }
    dialog.move_down();
    true
}
