//! Theme-picker input and transactional commit behavior.

use std::path::PathBuf;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use tokio::sync::mpsc;

use crate::command::SlashTranscriptEntry;
use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::i18n::t;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::ThemePickerOrigin;
use crate::state::ThemePickerState;
use crate::state::ui::Toast;
use crate::theme::ThemeSetting;

pub(crate) fn map_key(key: KeyEvent) -> Option<TuiCommand> {
    match key.code {
        KeyCode::Up => Some(TuiCommand::SurfacePrev),
        KeyCode::Down => Some(TuiCommand::SurfaceNext),
        KeyCode::Home => Some(TuiCommand::SurfaceJumpStart),
        KeyCode::End => Some(TuiCommand::SurfaceJumpEnd),
        KeyCode::Enter => Some(TuiCommand::SurfaceConfirm),
        KeyCode::Esc => Some(TuiCommand::Cancel),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiCommand::Cancel)
        }
        _ => None,
    }
}

pub(super) async fn confirm(
    state: &mut AppState,
    picker: ThemePickerState,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    confirm_with(state, picker, command_tx, crate::theme::save_theme_setting).await;
}

async fn confirm_with<F>(
    state: &mut AppState,
    picker: ThemePickerState,
    command_tx: &mpsc::Sender<UserCommand>,
    save: F,
) where
    F: FnOnce(&ThemeSetting) -> anyhow::Result<PathBuf>,
{
    let Some(choice) = state
        .ui
        .theme_state
        .choices
        .get(picker.selected.max(0) as usize)
        .cloned()
    else {
        match picker.origin {
            ThemePickerOrigin::Standalone => state.ui.finish_taken_modal(),
            ThemePickerOrigin::Settings(settings) => {
                state.ui.show_modal(ModalState::Settings(*settings));
            }
        }
        return;
    };

    if let Err(err) = state.ui.apply_theme_setting(choice.setting.clone()) {
        state.ui.add_toast(Toast::error(
            t!("toast.theme_apply_failed", error = err.to_string()).to_string(),
        ));
        state.ui.restore_modal(ModalState::ThemePicker(picker));
        return;
    }

    if let Err(err) = save(&choice.setting) {
        if let Err(rollback_err) = state
            .ui
            .apply_theme_setting(picker.original_setting.clone())
        {
            tracing::warn!(
                error = %rollback_err,
                "theme picker: failed to roll back after persistence failure"
            );
            state.ui.add_toast(Toast::error(
                t!(
                    "toast.theme_restore_failed",
                    error = rollback_err.to_string()
                )
                .to_string(),
            ));
        }
        state.ui.add_toast(Toast::error(
            t!("toast.theme_save_failed", error = err.to_string()).to_string(),
        ));
        state.ui.restore_modal(ModalState::ThemePicker(picker));
        return;
    }

    match picker.origin {
        ThemePickerOrigin::Standalone => {
            state.ui.finish_taken_modal();
            let entry = SlashTranscriptEntry::Result {
                name: "theme".to_string(),
                args: String::new(),
                text: format!("Theme set to {}", choice.label),
                is_error: false,
            };
            if let Some(session_id) = state.active_session_id() {
                let _ = command_tx
                    .send(UserCommand::PushSlashResult { session_id, entry })
                    .await;
            }
        }
        ThemePickerOrigin::Settings(settings) => {
            state.ui.show_modal(ModalState::Settings(*settings));
        }
    }
}

#[cfg(test)]
#[path = "theme_picker.test.rs"]
mod tests;
