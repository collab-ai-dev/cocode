//! Tabbed Settings modal behavior.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use crate::events::TuiCommand;
use crate::i18n::t;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::ui::Toast;
use crate::widgets::settings_panel::SettingsPanelState;
use crate::widgets::settings_panel::SettingsTab;

pub(crate) fn map_key(key: KeyEvent) -> Option<TuiCommand> {
    match key.code {
        KeyCode::Tab => Some(TuiCommand::SettingsNextTab),
        KeyCode::BackTab => Some(TuiCommand::SettingsPrevTab),
        KeyCode::Up => Some(TuiCommand::SurfacePrev),
        KeyCode::Down => Some(TuiCommand::SurfaceNext),
        KeyCode::Enter => Some(TuiCommand::SurfaceConfirm),
        KeyCode::Esc => Some(TuiCommand::Cancel),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiCommand::Cancel)
        }
        _ => None,
    }
}

pub(super) fn confirm(state: &mut AppState, mut s: SettingsPanelState) {
    if let SettingsTab::Display = s.active_tab {
        if s.is_syntax_highlighting_selected() {
            toggle_syntax_highlighting(state);
            s.set_display_settings(state.ui.display_settings.clone());
        } else if s.is_copy_full_response_selected() {
            toggle_copy_full_response(state);
            s.set_display_settings(state.ui.display_settings.clone());
        }
    }
    state.ui.restore_modal(ModalState::Settings(s));
}

pub(crate) fn toggle_syntax_highlighting(state: &mut AppState) {
    if let Some(source) = state
        .ui
        .display_settings
        .syntax_highlighting_editability
        .overriding_source()
    {
        state.ui.add_toast(Toast::warning(
            t!(
                "toast.syntax_highlighting_overridden",
                source = source.as_str()
            )
            .to_string(),
        ));
        return;
    }

    let next = state
        .ui
        .display_settings
        .clone()
        .with_syntax_highlighting(state.ui.display_settings.syntax_highlighting.toggle());

    let disabled = next.syntax_highlighting.is_disabled();
    match coco_config::global_config::write_user_setting(
        coco_config::settings::SYNTAX_HIGHLIGHTING_DISABLED_KEY,
        serde_json::json!(disabled),
    ) {
        Ok(path) => {
            let status = crate::widgets::settings_panel::syntax_highlighting_status(
                next.syntax_highlighting,
            );
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

pub(super) fn item_count(s: &SettingsPanelState) -> usize {
    match s.active_tab {
        SettingsTab::Display => s.display_item_count(),
        SettingsTab::OutputStyle => s.output_styles.len(),
        SettingsTab::Permissions => s.permission_rules.len(),
        SettingsTab::About => 0,
    }
}
