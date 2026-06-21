//! Key dispatch for the `/agents` 2-tab dialog.
//!
//! - Tab switch: `←` / `→`.
//! - List nav: `↑` / `↓` per tab.
//! - Running tab `X` (case-insensitive): cancel highlighted task.
//! - Library tab `Enter` on an editable agent row → fork `$EDITOR`
//!   against the markdown source; on `Create new` → open the inline
//!   4-step wizard (name → description → source → confirm); on the
//!   wizard's final step → dispatch `UserCommand::CreateAgent`.
//! - Library tab `d` on an editable agent row → delete + reload.
//! - `Esc` — inside the wizard goes back one step (or cancels on
//!   step 1); otherwise closes the dialog.
//!
//! The wizard collects name + description + source + (auto-) color.
//! Tools / model / memory default in the template and land in
//! `$EDITOR` for the user to tune.
//!
//! Every keystroke handler returns a `bool` propagated through
//! [`Handled::Yes`]: `true` ⇒ state actually changed (request a
//! redraw); `false` ⇒ the key was swallowed but produced no visible
//! effect, so the caller can skip the frame.

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::state::AgentsDialogState;
use crate::state::AgentsDialogTab;
use crate::state::AppState;
use crate::state::CreateWizardState;
use crate::state::CreateWizardStep;
use crate::state::LibraryRow;
use crate::state::LibraryToastKind;
use crate::state::ModalState;
use crate::state::SubagentStatus;
use crate::state::WizardError;
use crate::state::is_valid_desc_char;
use crate::state::is_valid_name_char;
use crate::state::resolve_create_target;
use crate::state::validate_agent_name;

/// Outcome of [`intercept`].
pub(super) enum Handled {
    Yes(bool),
    No,
}

pub(super) async fn intercept(
    state: &mut AppState,
    cmd: &TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> Handled {
    // Tightly scope the immutable borrow so the `intercept_wizard`
    // re-borrow below isn't conflated with it by NLL — a future
    // change that reads `dialog` after the wizard branch shouldn't
    // implicitly extend the borrow lifetime.
    let in_wizard = match state.ui.modal.as_ref() {
        Some(ModalState::AgentsDialog(d)) => d.is_in_wizard(),
        _ => return Handled::No,
    };

    if in_wizard {
        return intercept_wizard(state, cmd, command_tx).await;
    }

    // A pending delete confirmation captures all keys until the user
    // confirms or cancels — nav/tab-switch must not slip through.
    let in_delete_confirm = match state.ui.modal.as_ref() {
        Some(ModalState::AgentsDialog(d)) => d.pending_delete.is_some(),
        _ => false,
    };
    if in_delete_confirm {
        return intercept_delete_confirm(state, cmd, command_tx).await;
    }

    match cmd {
        TuiCommand::CursorLeft => Handled::Yes(cycle_tab(state, -1)),
        TuiCommand::CursorRight => Handled::Yes(cycle_tab(state, 1)),
        TuiCommand::CursorUp => Handled::Yes(move_cursor(state, -1)),
        TuiCommand::CursorDown => Handled::Yes(move_cursor(state, 1)),
        TuiCommand::Cancel => {
            state.ui.dismiss_modal();
            Handled::Yes(true)
        }
        TuiCommand::SubmitInput => {
            on_submit(state, command_tx).await;
            Handled::Yes(true)
        }
        TuiCommand::InsertChar(c) if c.eq_ignore_ascii_case(&'x') => {
            cancel_focused_running_task(state, command_tx).await;
            Handled::Yes(true)
        }
        TuiCommand::InsertChar(c) if c.eq_ignore_ascii_case(&'d') => {
            arm_delete_focused_library_agent(state);
            Handled::Yes(true)
        }
        _ => Handled::No,
    }
}

async fn on_submit(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    let tab = match state.ui.modal.as_ref() {
        Some(ModalState::AgentsDialog(d)) => d.selected_tab,
        _ => return,
    };
    match tab {
        AgentsDialogTab::Library => {
            on_library_submit(state, command_tx).await;
        }
        AgentsDialogTab::Running => {
            // Running tab Enter currently no-ops; detail is surfaced
            // via `Ctrl+T` on the inline activity panel.
        }
    }
}

async fn on_library_submit(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    match classify_library_submit(state) {
        LibrarySubmitAction::Edit(path) => {
            let _ = command_tx.send(UserCommand::OpenAgentEditor { path }).await;
        }
        LibrarySubmitAction::OpenWizard => {
            if let Some(dialog) = dialog_mut(state) {
                dialog.open_wizard();
            }
        }
        LibrarySubmitAction::Toast(kind) => {
            state
                .ui
                .add_toast(crate::state::ui::Toast::info(render_library_toast(kind)));
        }
        LibrarySubmitAction::None => {}
    }
}

enum LibrarySubmitAction {
    Edit(PathBuf),
    OpenWizard,
    Toast(LibraryToastKind),
    None,
}

fn classify_library_submit(state: &AppState) -> LibrarySubmitAction {
    let dialog = match state.ui.modal.as_ref() {
        Some(ModalState::AgentsDialog(d)) => d,
        _ => return LibrarySubmitAction::None,
    };
    match dialog.focused_library() {
        Some(LibraryRow::Agent {
            source_path: Some(path),
            is_builtin: false,
            ..
        }) => LibrarySubmitAction::Edit(path.clone()),
        Some(LibraryRow::Agent {
            is_builtin: true, ..
        }) => LibrarySubmitAction::Toast(LibraryToastKind::BuiltinReadOnly),
        Some(LibraryRow::Agent {
            source_path: None, ..
        }) => LibrarySubmitAction::Toast(LibraryToastKind::NoFile),
        Some(LibraryRow::CreateNew) => LibrarySubmitAction::OpenWizard,
        Some(LibraryRow::SourceHeader { .. }) | None => LibrarySubmitAction::None,
    }
}

/// Compile-time exhaustive toast renderer — adding a [`LibraryToastKind`]
/// variant requires an arm here, so a new toast can never silently
/// surface as its enum name.
fn render_library_toast(kind: LibraryToastKind) -> String {
    match kind {
        LibraryToastKind::BuiltinReadOnly => {
            crate::i18n::t!("dialog.agents_builtin_readonly").to_string()
        }
        LibraryToastKind::NoFile => crate::i18n::t!("dialog.agents_no_file").to_string(),
    }
}

fn arm_delete_focused_library_agent(state: &mut AppState) {
    let path_to_delete = {
        let dialog = match state.ui.modal.as_ref() {
            Some(ModalState::AgentsDialog(d)) => d,
            _ => return,
        };
        if dialog.selected_tab != AgentsDialogTab::Library {
            return;
        }
        match dialog.focused_library() {
            Some(LibraryRow::Agent {
                source_path: Some(path),
                is_builtin: false,
                ..
            }) => Some(path.clone()),
            _ => None,
        }
    };
    if let Some(path) = path_to_delete
        && let Some(ModalState::AgentsDialog(dialog)) = state.ui.modal.as_mut()
    {
        // Arm the confirmation prompt; the actual `DeleteAgentFile`
        // dispatch happens only after the user confirms in
        // `intercept_delete_confirm`.
        dialog.pending_delete = Some(path);
    }
}

/// Handle keys while a delete confirmation is armed: `y` / Enter
/// dispatches the destructive command, `n` / Esc cancels. All other
/// keys are swallowed (no nav leakage) without a redraw.
async fn intercept_delete_confirm(
    state: &mut AppState,
    cmd: &TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> Handled {
    let confirm = matches!(cmd, TuiCommand::SubmitInput)
        || matches!(cmd, TuiCommand::InsertChar(c) if c.eq_ignore_ascii_case(&'y'));
    let cancel = matches!(cmd, TuiCommand::Cancel)
        || matches!(cmd, TuiCommand::InsertChar(c) if c.eq_ignore_ascii_case(&'n'));

    let pending = match state.ui.modal.as_mut() {
        Some(ModalState::AgentsDialog(d)) => d.pending_delete.take_if(|_| confirm || cancel),
        _ => return Handled::No,
    };

    match pending {
        Some(path) if confirm => {
            state.ui.add_toast(crate::state::ui::Toast::info(
                crate::i18n::t!(
                    "dialog.agents_deleted_toast",
                    path = path.display().to_string().as_str()
                )
                .to_string(),
            ));
            let _ = command_tx.send(UserCommand::DeleteAgentFile { path }).await;
            Handled::Yes(true)
        }
        // cancel (pending was taken and dropped) or a swallowed key.
        _ => Handled::Yes(confirm || cancel),
    }
}

// ── Inline create wizard dispatch ──────────────────────────────────

async fn intercept_wizard(
    state: &mut AppState,
    cmd: &TuiCommand,
    command_tx: &mpsc::Sender<UserCommand>,
) -> Handled {
    match cmd {
        TuiCommand::Cancel => Handled::Yes(wizard_back_or_cancel(state)),
        TuiCommand::SubmitInput => {
            wizard_advance(state, command_tx).await;
            Handled::Yes(true)
        }
        TuiCommand::DeleteBackward => Handled::Yes(wizard_backspace(state)),
        TuiCommand::InsertChar(c) => Handled::Yes(wizard_input_char(state, *c)),
        TuiCommand::CursorUp => Handled::Yes(wizard_source_nav(state, -1)),
        TuiCommand::CursorDown => Handled::Yes(wizard_source_nav(state, 1)),
        TuiCommand::CursorLeft => Handled::Yes(wizard_caret_left(state)),
        TuiCommand::CursorRight => Handled::Yes(wizard_caret_right(state)),
        TuiCommand::CursorHome => Handled::Yes(wizard_caret_home(state)),
        TuiCommand::CursorEnd => Handled::Yes(wizard_caret_end(state)),
        TuiCommand::DeleteForward => Handled::Yes(wizard_delete_forward(state)),
        // Anything else stays inside the wizard but doesn't ask for
        // a redraw — prevents stray keys like `x` / `d` from
        // burning frame budget on no-ops.
        _ => Handled::Yes(false),
    }
}

fn wizard_mut(state: &mut AppState) -> Option<&mut CreateWizardState> {
    match state.ui.modal.as_mut() {
        Some(ModalState::AgentsDialog(d)) => d.wizard.as_mut(),
        _ => None,
    }
}

fn wizard_ref(state: &AppState) -> Option<&CreateWizardState> {
    match state.ui.modal.as_ref() {
        Some(ModalState::AgentsDialog(d)) => d.wizard.as_ref(),
        _ => None,
    }
}

fn wizard_input_char(state: &mut AppState, c: char) -> bool {
    let step = match wizard_ref(state) {
        Some(w) => w.step,
        None => return false,
    };
    match step {
        CreateWizardStep::Name => {
            // Only accept characters legal in the final identifier.
            // Whitespace / punctuation / non-ASCII letters get rejected
            // immediately rather than bouncing off a deferred validation.
            if !is_valid_name_char(c) {
                return false;
            }
            if let Some(w) = wizard_mut(state) {
                w.error = None;
                w.name.insert_char(c);
                return true;
            }
            false
        }
        CreateWizardStep::Description => {
            if !is_valid_desc_char(c) {
                return false;
            }
            if let Some(w) = wizard_mut(state) {
                w.error = None;
                w.description.insert_char(c);
                return true;
            }
            false
        }
        CreateWizardStep::Source | CreateWizardStep::Confirm => {
            // Text entry is meaningless here — return `false` so the
            // caller doesn't schedule a redraw for a no-op.
            false
        }
    }
}

fn wizard_backspace(state: &mut AppState) -> bool {
    let Some(w) = wizard_mut(state) else {
        return false;
    };
    let Some(field) = w.active_field_mut() else {
        return false;
    };
    if field.cursor == 0 {
        return false;
    }
    field.delete_back();
    w.error = None;
    true
}

fn wizard_delete_forward(state: &mut AppState) -> bool {
    let Some(w) = wizard_mut(state) else {
        return false;
    };
    let Some(field) = w.active_field_mut() else {
        return false;
    };
    let len_before = field.text.len();
    field.delete_forward();
    let changed = field.text.len() != len_before;
    if changed {
        w.error = None;
    }
    changed
}

fn wizard_caret_left(state: &mut AppState) -> bool {
    let Some(w) = wizard_mut(state) else {
        return false;
    };
    let Some(field) = w.active_field_mut() else {
        return false;
    };
    if field.cursor == 0 {
        return false;
    }
    field.move_left();
    true
}

fn wizard_caret_right(state: &mut AppState) -> bool {
    let Some(w) = wizard_mut(state) else {
        return false;
    };
    let Some(field) = w.active_field_mut() else {
        return false;
    };
    if field.cursor >= field.char_len() {
        return false;
    }
    field.move_right();
    true
}

fn wizard_caret_home(state: &mut AppState) -> bool {
    let Some(w) = wizard_mut(state) else {
        return false;
    };
    let Some(field) = w.active_field_mut() else {
        return false;
    };
    if field.cursor == 0 {
        return false;
    }
    field.move_home();
    true
}

fn wizard_caret_end(state: &mut AppState) -> bool {
    let Some(w) = wizard_mut(state) else {
        return false;
    };
    let Some(field) = w.active_field_mut() else {
        return false;
    };
    let end = field.char_len();
    if field.cursor == end {
        return false;
    }
    field.move_end();
    true
}

fn wizard_back_or_cancel(state: &mut AppState) -> bool {
    // Snapshot the step up front so the borrow on `wizard` ends
    // before we potentially call `close_wizard` on the dialog. Keeps
    // NLL straight and removes a re-borrow pothole.
    let step = match wizard_ref(state) {
        Some(w) => w.step,
        None => return false,
    };
    match step {
        CreateWizardStep::Name => {
            if let Some(dialog) = dialog_mut(state) {
                dialog.close_wizard();
                return true;
            }
            false
        }
        CreateWizardStep::Description => {
            if let Some(w) = wizard_mut(state) {
                w.step = CreateWizardStep::Name;
                w.error = None;
                return true;
            }
            false
        }
        CreateWizardStep::Source => {
            if let Some(w) = wizard_mut(state) {
                w.step = CreateWizardStep::Description;
                w.error = None;
                return true;
            }
            false
        }
        CreateWizardStep::Confirm => {
            if let Some(w) = wizard_mut(state) {
                w.step = CreateWizardStep::Source;
                w.error = None;
                return true;
            }
            false
        }
    }
}

fn wizard_source_nav(state: &mut AppState, delta: i32) -> bool {
    let Some(w) = wizard_mut(state) else {
        return false;
    };
    if w.step != CreateWizardStep::Source {
        return false;
    }
    let prev = w.source;
    w.source = w.source.cycled(delta);
    prev != w.source
}

async fn wizard_advance(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    let step = match wizard_ref(state) {
        Some(w) => w.step,
        None => return,
    };
    match step {
        CreateWizardStep::Name => {
            let name_snapshot = match wizard_ref(state) {
                Some(w) => w.name.text.trim().to_string(),
                None => return,
            };
            match validate_agent_name(&name_snapshot) {
                Ok(()) => {
                    if let Some(w) = wizard_mut(state) {
                        // Trim the field in place + move cursor to end
                        // so the next Esc/Enter sees the normalized
                        // value.
                        w.name.text = name_snapshot;
                        w.name.cursor = w.name.char_len();
                        w.step = CreateWizardStep::Description;
                        w.error = None;
                    }
                }
                Err(err) => {
                    if let Some(w) = wizard_mut(state) {
                        w.error = Some(err);
                    }
                }
            }
        }
        CreateWizardStep::Description => {
            let desc = match wizard_ref(state) {
                Some(w) => w.description.text.trim().to_string(),
                None => return,
            };
            if desc.is_empty() {
                if let Some(w) = wizard_mut(state) {
                    w.error = Some(WizardError::DescEmpty);
                }
                return;
            }
            if let Some(w) = wizard_mut(state) {
                w.description.text = desc;
                w.description.cursor = w.description.char_len();
                w.step = CreateWizardStep::Source;
                w.error = None;
            }
        }
        CreateWizardStep::Source => {
            // Source → Confirm gives the user one last look before
            // anything hits the filesystem.
            if let Some(w) = wizard_mut(state) {
                w.step = CreateWizardStep::Confirm;
                w.error = None;
            }
        }
        CreateWizardStep::Confirm => {
            wizard_finalize(state, command_tx).await;
        }
    }
}

async fn wizard_finalize(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    let cwd = std::env::current_dir().unwrap_or_default();
    let config_home = coco_config::global_config::config_home();
    wizard_finalize_with(state, command_tx, &cwd, &config_home).await;
}

/// Pure-paths variant of [`wizard_finalize`] used by both the live
/// dispatch (which reads `std::env::current_dir()` /
/// `config_home()`) and unit tests (which pass `tempfile::TempDir`
/// paths). Keeping the global-env lookup in the thin wrapper above
/// means the testable surface never touches process state.
async fn wizard_finalize_with(
    state: &mut AppState,
    command_tx: &mpsc::Sender<UserCommand>,
    cwd: &std::path::Path,
    config_home: &std::path::Path,
) {
    let (name, description, source) = match wizard_ref(state) {
        Some(w) => (
            w.name.text.clone(),
            w.description.text.clone(),
            w.source.as_agent_source(),
        ),
        None => return,
    };

    // Pre-flight on the blocking pool — `path.exists()` is the only
    // syscall, but pushing it off the event loop keeps the TUI
    // latency-flat on slow filesystems.
    let preflight = tokio::task::spawn_blocking({
        let name = name.clone();
        let cwd = cwd.to_path_buf();
        let config_home = config_home.to_path_buf();
        move || resolve_create_target(source, &name, &cwd, &config_home)
    })
    .await;
    let target = match preflight {
        Ok(Ok(path)) => path,
        Ok(Err(err)) => {
            if let Some(w) = wizard_mut(state) {
                w.error = Some(err);
            }
            return;
        }
        Err(join_err) => {
            tracing::warn!(
                target: "coco::agents",
                error = %join_err,
                "wizard pre-flight spawn_blocking panicked"
            );
            return;
        }
    };
    tracing::debug!(
        target: "coco::agents",
        path = %target.display(),
        "wizard pre-flight cleared; dispatching CreateAgent"
    );

    if let Some(dialog) = dialog_mut(state) {
        dialog.close_wizard();
    }
    let _ = command_tx
        .send(UserCommand::CreateAgent {
            name,
            description,
            source,
        })
        .await;
}

fn dialog_mut(state: &mut AppState) -> Option<&mut AgentsDialogState> {
    match state.ui.modal.as_mut() {
        Some(ModalState::AgentsDialog(d)) => Some(d),
        _ => None,
    }
}

fn cycle_tab(state: &mut AppState, delta: i32) -> bool {
    let Some(dialog) = dialog_mut(state) else {
        return false;
    };
    let prev = dialog.selected_tab;
    dialog.selected_tab = dialog.selected_tab.cycled(delta);
    prev != dialog.selected_tab
}

fn move_cursor(state: &mut AppState, delta: i32) -> bool {
    // Cursor logic is per-tab. Running cursor is bounded by the live
    // active-tasks count derived from `session.subagents`; Library
    // cursor walks selectable rows.
    let tab = match state.ui.modal.as_ref() {
        Some(ModalState::AgentsDialog(d)) => d.selected_tab,
        _ => return false,
    };
    match tab {
        AgentsDialogTab::Running => {
            let active_count = state
                .session
                .subagents
                .iter()
                .filter(|s| s.status == SubagentStatus::Running)
                .count();
            let Some(dialog) = dialog_mut(state) else {
                return false;
            };
            if active_count == 0 {
                let changed = dialog.running_cursor != 0;
                dialog.running_cursor = 0;
                return changed;
            }
            let next =
                (dialog.running_cursor as i32 + delta).rem_euclid(active_count as i32) as usize;
            let changed = next != dialog.running_cursor;
            dialog.running_cursor = next;
            changed
        }
        AgentsDialogTab::Library => {
            let Some(dialog) = dialog_mut(state) else {
                return false;
            };
            let prev = dialog.library_cursor;
            dialog.nav_library(delta);
            prev != dialog.library_cursor
        }
    }
}

async fn cancel_focused_running_task(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    let Some(task_id) = focused_running_task_id(state) else {
        return;
    };
    let _ = command_tx
        .send(UserCommand::CancelSubagent { task_id })
        .await;
}

fn focused_running_task_id(state: &AppState) -> Option<String> {
    let dialog = match state.ui.modal.as_ref()? {
        ModalState::AgentsDialog(d) => d,
        _ => return None,
    };
    if dialog.selected_tab != AgentsDialogTab::Running {
        return None;
    }
    let active: Vec<&_> = state
        .session
        .subagents
        .iter()
        .filter(|s| s.status == SubagentStatus::Running)
        .collect();
    let row = active.get(dialog.running_cursor)?;
    // SubagentInstance.agent_id is the TaskManager registry key.
    Some(row.agent_id.clone())
}

#[cfg(test)]
#[path = "agents_dialog.test.rs"]
mod tests;
