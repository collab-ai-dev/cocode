//! Key dispatch for the `/permissions` rule-editor overlay.
//!
//! Three nested modes share the overlay:
//!   - **list** — ←/→ switch tab, ↑/↓ select a row, Enter acts on the
//!     focused row (open the add form / open a delete confirm), Esc closes.
//!   - **add form** — type a rule pattern or directory path, Enter advances
//!     to the destination selector, Enter again persists via
//!     `UserCommand::ApplyPermissionUpdate`.
//!   - **delete confirm** — ←/→ (or y/n) toggle Yes/No, Enter commits.
//!
//! Every persisted edit round-trips through the CLI, which re-emits
//! `OpenPermissionsEditor` so the list refreshes from disk — the overlay
//! never edits the on-disk truth in place.
//!
//! Handler return contract matches `agents_dialog`: `Handled::Yes(true)`
//! ⇒ state changed (redraw), `Handled::Yes(false)` ⇒ key swallowed but no
//! visible effect.

use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::state::AddForm;
use crate::state::AddStep;
use crate::state::AppState;
use crate::state::DeleteConfirm;
use crate::state::DeleteTarget;
use crate::state::EditorDestination;
use crate::state::ModalState;
use crate::state::PermDirRow;
use crate::state::PermEditorError;
use crate::state::PermEditorFocused;
use crate::state::PermRuleRow;
use crate::state::PermissionsEditorState;
use crate::state::permissions_editor::source_destination;

use coco_types::PermissionRule;
use coco_types::PermissionUpdate;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

/// Keys for the `/permissions` rule editor. Maps to input-style commands
/// consumed directly by [`intercept`].
pub(crate) fn map_key(key: KeyEvent) -> Option<TuiCommand> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Left => Some(TuiCommand::CursorLeft),
        KeyCode::Right => Some(TuiCommand::CursorRight),
        KeyCode::Up => Some(TuiCommand::CursorUp),
        KeyCode::Down => Some(TuiCommand::CursorDown),
        KeyCode::Tab => Some(TuiCommand::CursorRight),
        KeyCode::BackTab => Some(TuiCommand::CursorLeft),
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

/// Outcome of [`intercept`].
pub(crate) enum Handled {
    Yes(bool),
    No,
}

pub(crate) async fn intercept(
    state: &mut AppState,
    cmd: &TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> Handled {
    // Scope the immutable borrow so the form re-borrows below stay clean
    // under NLL (same pattern as `agents_dialog::intercept`).
    let (in_add, in_delete) = match state.ui.modal.as_ref() {
        Some(ModalState::PermissionsEditor(e)) => {
            (e.add_form.is_some(), e.delete_confirm.is_some())
        }
        _ => return Handled::No,
    };

    if in_add {
        return intercept_add_form(state, cmd, command_tx).await;
    }
    if in_delete {
        return intercept_delete_confirm(state, cmd, command_tx).await;
    }

    match cmd {
        TuiCommand::CursorLeft => Handled::Yes(cycle_tab(state, -1)),
        TuiCommand::CursorRight => Handled::Yes(cycle_tab(state, 1)),
        TuiCommand::CursorUp => Handled::Yes(nav(state, -1)),
        TuiCommand::CursorDown => Handled::Yes(nav(state, 1)),
        TuiCommand::Cancel => {
            state.ui.dismiss_modal();
            Handled::Yes(true)
        }
        TuiCommand::SubmitInput => Handled::Yes(on_submit(state)),
        // Swallow stray keys (chars in list mode etc.) without burning a
        // frame on a no-op.
        _ => Handled::Yes(false),
    }
}

// ── Accessors ──────────────────────────────────────────────────────

fn editor_ref(state: &AppState) -> Option<&PermissionsEditorState> {
    match state.ui.modal.as_ref() {
        Some(ModalState::PermissionsEditor(e)) => Some(e),
        _ => None,
    }
}

fn editor_mut(state: &mut AppState) -> Option<&mut PermissionsEditorState> {
    match state.ui.modal.as_mut() {
        Some(ModalState::PermissionsEditor(e)) => Some(e),
        _ => None,
    }
}

// ── List mode ──────────────────────────────────────────────────────

fn cycle_tab(state: &mut AppState, delta: i32) -> bool {
    let Some(editor) = editor_mut(state) else {
        return false;
    };
    let prev = editor.selected_tab;
    editor.selected_tab = editor.selected_tab.cycled(delta);
    prev != editor.selected_tab
}

fn nav(state: &mut AppState, delta: i32) -> bool {
    let Some(editor) = editor_mut(state) else {
        return false;
    };
    editor.nav(delta)
}

/// Action chosen by Enter in list mode — computed under an immutable
/// borrow, applied under a mutable one.
enum SubmitAction {
    OpenAdd,
    DeleteRule(PermRuleRow),
    DeleteDir(PermDirRow),
    None,
}

fn on_submit(state: &mut AppState) -> bool {
    let action = {
        let Some(editor) = editor_ref(state) else {
            return false;
        };
        // Managed-policy lockdown: no add / delete.
        if editor.managed_only {
            return false;
        }
        match editor.focused() {
            PermEditorFocused::Add => SubmitAction::OpenAdd,
            PermEditorFocused::Rule(rule) if rule.is_editable() => {
                SubmitAction::DeleteRule(rule.clone())
            }
            PermEditorFocused::Dir(dir) if dir.is_editable() => {
                SubmitAction::DeleteDir(dir.clone())
            }
            // Read-only rows (policy / cwd / cli / session) — no action.
            PermEditorFocused::Rule(_) | PermEditorFocused::Dir(_) | PermEditorFocused::None => {
                SubmitAction::None
            }
        }
    };
    let Some(editor) = editor_mut(state) else {
        return false;
    };
    match action {
        SubmitAction::OpenAdd => {
            editor.add_form = Some(AddForm::new());
            true
        }
        SubmitAction::DeleteRule(rule) => {
            editor.delete_confirm = Some(DeleteConfirm {
                yes: false,
                target: DeleteTarget::Rule(rule),
            });
            true
        }
        SubmitAction::DeleteDir(dir) => {
            editor.delete_confirm = Some(DeleteConfirm {
                yes: false,
                target: DeleteTarget::Dir(dir),
            });
            true
        }
        SubmitAction::None => false,
    }
}

// ── Add form ───────────────────────────────────────────────────────

async fn intercept_add_form(
    state: &mut AppState,
    cmd: &TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> Handled {
    match cmd {
        TuiCommand::Cancel => Handled::Yes(add_form_back_or_cancel(state)),
        TuiCommand::SubmitInput => {
            add_form_advance(state, command_tx).await;
            Handled::Yes(true)
        }
        TuiCommand::InsertChar(c) => Handled::Yes(add_form_input_char(state, *c)),
        TuiCommand::DeleteBackward => Handled::Yes(add_form_backspace(state)),
        TuiCommand::DeleteForward => Handled::Yes(add_form_delete_forward(state)),
        TuiCommand::CursorLeft => Handled::Yes(add_form_caret_left(state)),
        TuiCommand::CursorRight => Handled::Yes(add_form_caret_right(state)),
        TuiCommand::CursorHome => Handled::Yes(add_form_caret_home(state)),
        TuiCommand::CursorEnd => Handled::Yes(add_form_caret_end(state)),
        TuiCommand::CursorUp => Handled::Yes(add_form_dest_nav(state, -1)),
        TuiCommand::CursorDown => Handled::Yes(add_form_dest_nav(state, 1)),
        _ => Handled::Yes(false),
    }
}

/// Route a bracketed paste into the add-rule form's text input. Paste travels a
/// separate event path (`TuiEvent::Paste`) that bypasses keybinding/modal
/// interception, so without this the clipboard text leaks into the main
/// composer hidden behind the overlay. Only consumes on the `Input` step (the
/// destination selector has no text field) and strips control chars so the rule
/// / path stays on one physical line — same posture as [`add_form_input_char`].
/// Returns `true` if consumed.
pub(crate) fn route_paste(state: &mut AppState, text: &str) -> bool {
    if add_form_step(state) != Some(AddStep::Input) {
        return false;
    }
    if let Some(form) = add_form_mut(state) {
        form.error = None;
        for c in text.chars().filter(|c| !c.is_control()) {
            form.input.insert_char(c);
        }
        return true;
    }
    false
}

fn add_form_mut(state: &mut AppState) -> Option<&mut AddForm> {
    editor_mut(state).and_then(|e| e.add_form.as_mut())
}

fn add_form_step(state: &AppState) -> Option<AddStep> {
    editor_ref(state)
        .and_then(|e| e.add_form.as_ref())
        .map(|f| f.step)
}

fn add_form_input_char(state: &mut AppState, c: char) -> bool {
    if add_form_step(state) != Some(AddStep::Input) {
        return false;
    }
    // Reject control chars (newlines/tabs) so the rule / path stays on one
    // physical line — same posture as the agents wizard description field.
    if c.is_control() {
        return false;
    }
    if let Some(form) = add_form_mut(state) {
        form.error = None;
        form.input.insert_char(c);
        return true;
    }
    false
}

fn add_form_backspace(state: &mut AppState) -> bool {
    if add_form_step(state) != Some(AddStep::Input) {
        return false;
    }
    if let Some(form) = add_form_mut(state) {
        if form.input.cursor == 0 {
            return false;
        }
        form.input.delete_back();
        form.error = None;
        return true;
    }
    false
}

fn add_form_delete_forward(state: &mut AppState) -> bool {
    if add_form_step(state) != Some(AddStep::Input) {
        return false;
    }
    if let Some(form) = add_form_mut(state) {
        let before = form.input.text.len();
        form.input.delete_forward();
        let changed = form.input.text.len() != before;
        if changed {
            form.error = None;
        }
        return changed;
    }
    false
}

fn add_form_caret_left(state: &mut AppState) -> bool {
    if add_form_step(state) != Some(AddStep::Input) {
        return false;
    }
    if let Some(form) = add_form_mut(state) {
        if form.input.cursor == 0 {
            return false;
        }
        form.input.move_left();
        return true;
    }
    false
}

fn add_form_caret_right(state: &mut AppState) -> bool {
    if add_form_step(state) != Some(AddStep::Input) {
        return false;
    }
    if let Some(form) = add_form_mut(state) {
        if form.input.cursor >= form.input.char_len() {
            return false;
        }
        form.input.move_right();
        return true;
    }
    false
}

fn add_form_caret_home(state: &mut AppState) -> bool {
    if add_form_step(state) != Some(AddStep::Input) {
        return false;
    }
    if let Some(form) = add_form_mut(state) {
        if form.input.cursor == 0 {
            return false;
        }
        form.input.move_home();
        return true;
    }
    false
}

fn add_form_caret_end(state: &mut AppState) -> bool {
    if add_form_step(state) != Some(AddStep::Input) {
        return false;
    }
    if let Some(form) = add_form_mut(state) {
        let end = form.input.char_len();
        if form.input.cursor == end {
            return false;
        }
        form.input.move_end();
        return true;
    }
    false
}

fn add_form_dest_nav(state: &mut AppState, delta: i32) -> bool {
    if add_form_step(state) != Some(AddStep::Destination) {
        return false;
    }
    if let Some(form) = add_form_mut(state) {
        return form.nav_destination(delta);
    }
    false
}

/// Esc inside the add form: back from Destination → Input, or close the
/// form from Input.
fn add_form_back_or_cancel(state: &mut AppState) -> bool {
    let step = match add_form_step(state) {
        Some(s) => s,
        None => return false,
    };
    match step {
        AddStep::Destination => {
            if let Some(form) = add_form_mut(state) {
                form.step = AddStep::Input;
                form.error = None;
                return true;
            }
            false
        }
        AddStep::Input => {
            if let Some(editor) = editor_mut(state) {
                editor.add_form = None;
                return true;
            }
            false
        }
    }
}

async fn add_form_advance(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    let step = match add_form_step(state) {
        Some(s) => s,
        None => return,
    };
    match step {
        AddStep::Input => {
            let trimmed = editor_ref(state)
                .and_then(|e| e.add_form.as_ref())
                .map(|f| f.input.text.trim().to_string())
                .unwrap_or_default();
            if trimmed.is_empty() {
                if let Some(form) = add_form_mut(state) {
                    form.error = Some(PermEditorError::EmptyInput);
                }
                return;
            }
            // Normalize the field in place + advance to the destination
            // selector.
            if let Some(form) = add_form_mut(state) {
                form.input.text = trimmed;
                form.input.cursor = form.input.char_len();
                form.step = AddStep::Destination;
                form.error = None;
            }
        }
        AddStep::Destination => {
            let update = build_add_update(state);
            if let Some(update) = update {
                let _ = command_tx
                    .send(UserCommand::ApplyPermissionUpdate { update })
                    .await;
            }
            // Close the form immediately; the CLI's refresh re-renders the
            // list with the persisted rule.
            if let Some(editor) = editor_mut(state) {
                editor.add_form = None;
            }
        }
    }
}

/// Build the `AddRules` / `AddDirectories` update from the current add
/// form + active tab.
fn build_add_update(state: &AppState) -> Option<PermissionUpdate> {
    let editor = editor_ref(state)?;
    let form = editor.add_form.as_ref()?;
    let input = form.input.text.trim();
    if input.is_empty() {
        return None;
    }
    let dest: EditorDestination = form.selected_destination();
    match editor.selected_tab.behavior() {
        Some(behavior) => {
            let value = coco_types::parse_rule_pattern(input);
            Some(PermissionUpdate::AddRules {
                rules: vec![PermissionRule {
                    source: dest.as_rule_source(),
                    behavior,
                    value,
                }],
                destination: dest.as_update_destination(),
            })
        }
        None => Some(PermissionUpdate::AddDirectories {
            directories: vec![input.to_string()],
            destination: dest.as_update_destination(),
        }),
    }
}

// ── Delete confirm ─────────────────────────────────────────────────

async fn intercept_delete_confirm(
    state: &mut AppState,
    cmd: &TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> Handled {
    match cmd {
        TuiCommand::Cancel => {
            if let Some(editor) = editor_mut(state) {
                editor.delete_confirm = None;
            }
            Handled::Yes(true)
        }
        TuiCommand::CursorLeft
        | TuiCommand::CursorRight
        | TuiCommand::CursorUp
        | TuiCommand::CursorDown => Handled::Yes(toggle_confirm(state)),
        TuiCommand::SubmitInput => {
            delete_confirm_submit(state, command_tx).await;
            Handled::Yes(true)
        }
        TuiCommand::InsertChar(c) if c.eq_ignore_ascii_case(&'y') => {
            set_confirm_yes(state, true);
            delete_confirm_submit(state, command_tx).await;
            Handled::Yes(true)
        }
        TuiCommand::InsertChar(c) if c.eq_ignore_ascii_case(&'n') => {
            if let Some(editor) = editor_mut(state) {
                editor.delete_confirm = None;
            }
            Handled::Yes(true)
        }
        _ => Handled::Yes(false),
    }
}

fn toggle_confirm(state: &mut AppState) -> bool {
    if let Some(editor) = editor_mut(state)
        && let Some(confirm) = editor.delete_confirm.as_mut()
    {
        confirm.yes = !confirm.yes;
        return true;
    }
    false
}

fn set_confirm_yes(state: &mut AppState, yes: bool) {
    if let Some(editor) = editor_mut(state)
        && let Some(confirm) = editor.delete_confirm.as_mut()
    {
        confirm.yes = yes;
    }
}

async fn delete_confirm_submit(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    let (yes, update) = {
        let Some(editor) = editor_ref(state) else {
            return;
        };
        let Some(confirm) = editor.delete_confirm.as_ref() else {
            return;
        };
        (confirm.yes, build_remove_update(&confirm.target))
    };
    if yes && let Some(update) = update {
        let _ = command_tx
            .send(UserCommand::ApplyPermissionUpdate { update })
            .await;
    }
    if let Some(editor) = editor_mut(state) {
        editor.delete_confirm = None;
    }
}

/// Build the `RemoveRules` / `RemoveDirectories` update for a delete
/// target. Returns `None` when the target's source isn't a writable
/// settings layer (defensive — the confirm only opens on editable rows).
fn build_remove_update(target: &DeleteTarget) -> Option<PermissionUpdate> {
    match target {
        DeleteTarget::Rule(rule) => {
            let dest = source_destination(rule.source)?;
            Some(PermissionUpdate::RemoveRules {
                rules: vec![PermissionRule {
                    source: rule.source,
                    behavior: rule.behavior,
                    value: rule.to_value(),
                }],
                destination: dest,
            })
        }
        DeleteTarget::Dir(dir) => {
            let dest = source_destination(dir.source)?;
            Some(PermissionUpdate::RemoveDirectories {
                directories: vec![dir.path.clone()],
                destination: dest,
            })
        }
    }
}

#[cfg(test)]
#[path = "permissions_editor.test.rs"]
mod tests;
