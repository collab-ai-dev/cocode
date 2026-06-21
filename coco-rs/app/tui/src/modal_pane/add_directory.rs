//! Key dispatch for the `/add-dir` (no-argument) directory-input overlay.
//!
//! A single text-input mode: type a directory path, Enter validates and adds it
//! to the session's working directories, Esc cancels. Validation runs
//! client-side (canonicalize + is-dir) so an invalid path keeps the form open
//! with an inline error; the actual add re-dispatches `/add-dir <path>` so the
//! validated session-add path in the CLI runner is reused, never duplicated.
//!
//! Handler return contract matches `permissions_editor`: `Handled::Yes(true)`
//! ⇒ state changed (redraw), `Handled::Yes(false)` ⇒ key swallowed but no
//! visible effect, `Handled::No` ⇒ not our modal, fall through.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::i18n::t;
use crate::state::AddDirectoryState;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::WizardTextField;

/// Keys for the `/add-dir` overlay — a single-line text field. Maps to the
/// input-style commands [`intercept`] consumes. Mirrors
/// `permissions_editor::map_key` minus the (unused) up/down destination nav.
pub(crate) fn map_key(key: KeyEvent) -> Option<TuiCommand> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Left => Some(TuiCommand::CursorLeft),
        KeyCode::Right => Some(TuiCommand::CursorRight),
        KeyCode::Enter => Some(TuiCommand::SubmitInput),
        KeyCode::Esc => Some(TuiCommand::Cancel),
        KeyCode::Backspace => Some(TuiCommand::DeleteBackward),
        KeyCode::Delete => Some(TuiCommand::DeleteForward),
        KeyCode::Home => Some(TuiCommand::CursorHome),
        KeyCode::End => Some(TuiCommand::CursorEnd),
        KeyCode::Char('c') if ctrl => Some(TuiCommand::Cancel),
        KeyCode::Char('a') if ctrl => Some(TuiCommand::CursorHome),
        KeyCode::Char('e') if ctrl => Some(TuiCommand::CursorEnd),
        KeyCode::Char(c) => Some(TuiCommand::InsertChar(c)),
        _ => None,
    }
}

/// Outcome of [`intercept`]. Mirrors `permissions_editor::Handled`.
pub(crate) enum Handled {
    Yes(bool),
    No,
}

pub(crate) async fn intercept(
    state: &mut AppState,
    cmd: &TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> Handled {
    if !matches!(state.ui.modal.as_ref(), Some(ModalState::AddDirectory(_))) {
        return Handled::No;
    }
    match cmd {
        TuiCommand::InsertChar(c) => Handled::Yes(insert_char(state, *c)),
        TuiCommand::DeleteBackward => Handled::Yes(edit(state, WizardTextField::delete_back)),
        TuiCommand::DeleteForward => Handled::Yes(edit(state, WizardTextField::delete_forward)),
        TuiCommand::CursorLeft => Handled::Yes(edit(state, WizardTextField::move_left)),
        TuiCommand::CursorRight => Handled::Yes(edit(state, WizardTextField::move_right)),
        TuiCommand::CursorHome => Handled::Yes(edit(state, WizardTextField::move_home)),
        TuiCommand::CursorEnd => Handled::Yes(edit(state, WizardTextField::move_end)),
        TuiCommand::SubmitInput => Handled::Yes(on_submit(state, command_tx).await),
        TuiCommand::Cancel => {
            on_cancel(state, command_tx).await;
            Handled::Yes(true)
        }
        // Swallow stray nav keys (Up/Down, Tab) so they don't leak to the chat
        // composer; no visible effect.
        _ => Handled::Yes(false),
    }
}

/// Route a bracketed paste into the `/add-dir` overlay input. Paste travels a
/// separate event path (`TuiEvent::Paste`) that bypasses keybinding/modal
/// interception, so without this the clipboard text leaks into the main
/// composer hidden behind the modal. Mirrors `question_free_text_paste`: only
/// consumes when our modal is active, strips control chars (the path stays on
/// one physical line, same as `insert_char`), and clears any stale error.
/// Returns `true` if consumed.
pub(crate) fn route_paste(state: &mut AppState, text: &str) -> bool {
    let Some(s) = add_state_mut(state) else {
        return false;
    };
    s.error = None;
    for c in text.chars().filter(|c| !c.is_control()) {
        s.input.insert_char(c);
    }
    true
}

fn add_state_mut(state: &mut AppState) -> Option<&mut AddDirectoryState> {
    match state.ui.modal.as_mut() {
        Some(ModalState::AddDirectory(s)) => Some(s),
        _ => None,
    }
}

/// Apply a cursor / edit op to the input field and clear any stale error.
fn edit(state: &mut AppState, f: impl FnOnce(&mut WizardTextField)) -> bool {
    if let Some(s) = add_state_mut(state) {
        s.error = None;
        f(&mut s.input);
        true
    } else {
        false
    }
}

fn insert_char(state: &mut AppState, c: char) -> bool {
    // Reject control chars (newlines/tabs) so the path stays on one physical
    // line — same posture as the permissions editor add form.
    if c.is_control() {
        return false;
    }
    edit(state, |f| f.insert_char(c))
}

async fn on_submit(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) -> bool {
    let path = match add_state_mut(state) {
        Some(s) => s.input.text.trim().to_string(),
        None => return false,
    };
    if path.is_empty() {
        if let Some(s) = add_state_mut(state) {
            s.error = Some(t!("dialog.add_dir_err_empty").to_string());
        }
        return true;
    }
    match validate_directory(&path) {
        Ok(()) => {
            state.ui.dismiss_modal();
            // Re-dispatch the validated path through the normal slash path so
            // the CLI runner's session-add + confirmation message own the add.
            if let Ok(name) = crate::state::SlashCommandName::new("add-dir") {
                let _ = command_tx
                    .send(UserCommand::ExecuteSlashCommand { name, args: path })
                    .await;
            }
            true
        }
        Err(msg) => {
            if let Some(s) = add_state_mut(state) {
                s.error = Some(msg);
            }
            true
        }
    }
}

async fn on_cancel(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    state.ui.dismiss_modal();
    let cancelled = t!("dialog.add_dir_cancelled").to_string();
    let messages = coco_messages::build_slash_command_messages(
        "add-dir", /*args*/ "", &cancelled, /*is_sensitive*/ false,
    );
    let _ = command_tx
        .send(UserCommand::PushSlashResult { messages })
        .await;
}

/// Mirror of `commands::add_dir_handler` validation so the overlay can give
/// inline feedback before re-dispatching. The handler re-validates as the
/// authority — this is purely for UX.
fn validate_directory(path: &str) -> Result<(), String> {
    match std::path::PathBuf::from(path).canonicalize() {
        Ok(abs) if abs.is_dir() => Ok(()),
        Ok(abs) => Err(t!(
            "dialog.add_dir_err_not_dir",
            path = abs.display().to_string()
        )
        .to_string()),
        Err(e) => Err(t!(
            "dialog.add_dir_err_invalid",
            path = path,
            error = e.to_string()
        )
        .to_string()),
    }
}

#[cfg(test)]
#[path = "add_directory.test.rs"]
mod tests;
