//! Searchable settings-browser behavior.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use crate::events::TuiCommand;
use crate::i18n::t;
use crate::settings_registry::SettingAction;
use crate::settings_registry::SettingId;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::ui::Toast;
use crate::widgets::settings_panel::SettingsPanelState;

pub(crate) fn map_key(key: KeyEvent) -> Option<TuiCommand> {
    match key.code {
        KeyCode::Up => Some(TuiCommand::SurfacePrev),
        KeyCode::Down => Some(TuiCommand::SurfaceNext),
        KeyCode::Home => Some(TuiCommand::SurfaceJumpStart),
        KeyCode::End => Some(TuiCommand::SurfaceJumpEnd),
        KeyCode::Enter => Some(TuiCommand::SurfaceConfirm),
        KeyCode::Backspace => Some(TuiCommand::SurfaceFilterBackspace),
        KeyCode::Esc => Some(TuiCommand::Cancel),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiCommand::Cancel)
        }
        KeyCode::Char(c) if matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            Some(TuiCommand::SurfaceFilter(c))
        }
        _ => None,
    }
}

/// Route bracketed paste and IME commits into the active settings filter.
/// Paste bypasses the keybinding path, so it must be consumed here to avoid
/// leaking text into the composer hidden behind the modal.
pub(crate) fn route_paste(state: &mut AppState, text: &str) -> bool {
    let Some(ModalState::Settings(settings)) = state.ui.modal.as_mut() else {
        return false;
    };
    for c in text.chars().filter(|c| !c.is_control()) {
        settings.insert_filter(c);
    }
    true
}

pub(super) fn confirm(state: &mut AppState, s: SettingsPanelState) {
    let Some(meta) = s.selected_setting().copied() else {
        state.ui.restore_modal(ModalState::Settings(s));
        return;
    };

    match meta.action {
        SettingAction::OpenThemePicker => {
            crate::update::show::open_theme_picker_from_settings(state, s);
            return;
        }
        SettingAction::CycleSyntaxHighlighting => {
            toggle_syntax_highlighting(state);
        }
        SettingAction::ToggleCopyFullResponse => {
            toggle_copy_full_response(state);
        }
        SettingAction::ReadOnly => {
            if let Some(message) = setting_override_message(state, meta.id) {
                state.ui.add_toast(Toast::warning(message));
            } else {
                let message = t!("toast.settings_edit_in_file", key = meta.id.key()).to_string();
                state.ui.add_toast(Toast::info(message));
            }
        }
    }
    state.ui.restore_modal(ModalState::Settings(s));
}

pub(crate) fn toggle_syntax_highlighting(state: &mut AppState) {
    if let Some(message) = setting_override_message(state, SettingId::SyntaxHighlighting) {
        state.ui.add_toast(Toast::warning(message));
        return;
    }

    let next = state
        .ui
        .display_settings
        .clone()
        .with_syntax_highlighting(state.ui.display_settings.syntax_highlighting.cycle());

    let level = crate::display_settings::syntax_highlighting_to_level(next.syntax_highlighting);
    match coco_config::global_config::write_user_setting(
        coco_config::settings::SYNTAX_HIGHLIGHTING_KEY,
        serde_json::json!(level),
    ) {
        Ok(path) => {
            let status =
                crate::presentation::settings::syntax_highlighting_status(next.syntax_highlighting);
            state.ui.apply_display_settings(next);
            let path_text = path.display().to_string();
            state.ui.add_toast(Toast::success(
                t!(
                    "toast.syntax_highlighting_saved",
                    status = status.as_str(),
                    path = path_text.as_str()
                )
                .to_string(),
            ));
        }
        Err(err) => state.ui.add_toast(Toast::error(
            t!(
                "toast.syntax_highlighting_save_failed",
                error = err.to_string().as_str()
            )
            .to_string(),
        )),
    }
}

fn toggle_copy_full_response(state: &mut AppState) {
    if let Some(message) = setting_override_message(state, SettingId::CopyFullResponse) {
        state.ui.add_toast(Toast::warning(message));
        return;
    }
    let enabled = !state.ui.display_settings.copy_full_response;
    let next = state
        .ui
        .display_settings
        .clone()
        .with_copy_full_response(enabled);

    match coco_config::global_config::write_user_setting(
        coco_config::settings::COPY_FULL_RESPONSE_KEY,
        serde_json::json!(enabled),
    ) {
        Ok(path) => {
            state.ui.apply_display_settings(next);
            let status = if enabled {
                t!("settings.enabled")
            } else {
                t!("settings.disabled")
            };
            let path_text = path.display().to_string();
            state.ui.add_toast(Toast::success(
                t!(
                    "toast.copy_full_response_saved",
                    status = status.as_ref(),
                    path = path_text.as_str()
                )
                .to_string(),
            ));
        }
        Err(err) => state.ui.add_toast(Toast::error(
            t!(
                "toast.copy_preference_save_failed",
                error = err.to_string().as_str()
            )
            .to_string(),
        )),
    }
}

pub(crate) fn setting_override_message(state: &AppState, id: SettingId) -> Option<String> {
    state
        .ui
        .display_settings
        .overriding_source(id)
        .map(|source| {
            t!(
                "toast.settings_overridden",
                key = id.key(),
                source = source.as_str()
            )
            .to_string()
        })
}

pub(super) fn item_count(s: &SettingsPanelState) -> usize {
    s.filtered_settings().len()
}

#[cfg(test)]
#[path = "settings.test.rs"]
mod tests;
