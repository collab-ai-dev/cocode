use super::*;
use crate::state::ThemePickerOrigin;
use coco_config::SettingSource;
use coco_config::Settings;
use coco_config::SettingsWithSource;
use std::collections::HashMap;

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn test_settings_map_key_uses_type_to_filter() {
    assert!(matches!(
        map_key(press(KeyCode::Char('m'))),
        Some(TuiCommand::SurfaceFilter('m'))
    ));
    assert!(matches!(
        map_key(press(KeyCode::Backspace)),
        Some(TuiCommand::SurfaceFilterBackspace)
    ));
    assert!(matches!(
        map_key(press(KeyCode::Home)),
        Some(TuiCommand::SurfaceJumpStart)
    ));
}

#[test]
fn test_item_count_tracks_filtered_registry() {
    let mut state = SettingsPanelState::default();
    let all = item_count(&state);
    state.filter = "telemetry".to_string();

    assert!(all > item_count(&state));
    assert_eq!(item_count(&state), 2);
}

#[test]
fn test_route_paste_filters_settings_and_strips_controls() {
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::Settings(SettingsPanelState::default()));

    assert!(route_paste(&mut state, "语法\n高亮"));
    let Some(ModalState::Settings(settings)) = state.ui.modal.as_ref() else {
        panic!("settings modal should remain open");
    };
    assert_eq!(settings.filter, "语法高亮");
}

#[test]
fn test_route_paste_ignored_without_settings_modal() {
    let mut state = AppState::new();
    assert!(!route_paste(&mut state, "theme"));
}

#[test]
fn theme_row_opens_picker_and_preserves_parent_view_state() {
    let mut state = AppState::new();
    let panel = SettingsPanelState {
        filter: "theme".to_string(),
        selected: 0,
    };

    confirm(&mut state, panel);

    let Some(ModalState::ThemePicker(picker)) = state.ui.modal.as_ref() else {
        panic!("theme row should open the theme picker")
    };
    let ThemePickerOrigin::Settings(parent) = &picker.origin else {
        panic!("settings should be retained as the picker parent")
    };
    assert_eq!(parent.filter, "theme");
    assert_eq!(parent.selected, 0);
    assert_eq!(picker.original_setting, state.ui.theme_state.setting);
    assert_eq!(
        state
            .ui
            .theme_state
            .choices
            .get(picker.selected as usize)
            .map(|choice| &choice.setting),
        Some(&state.ui.theme_state.setting)
    );
}

#[test]
fn copy_toggle_does_not_write_through_a_higher_priority_owner() {
    let mut state = AppState::new();
    let settings = SettingsWithSource {
        merged: Settings {
            copy_full_response: true,
            ..Settings::default()
        },
        per_source: HashMap::from([(
            SettingSource::Project,
            serde_json::json!({ "copy_full_response": true }),
        )]),
        source_paths: HashMap::new(),
    };
    state.ui.display_settings =
        crate::display_settings::DisplaySettings::from_settings_with_sources(&settings);

    toggle_copy_full_response(&mut state);

    assert!(state.ui.display_settings.copy_full_response);
    assert_eq!(state.ui.toasts.len(), 1);
    assert_eq!(
        state.ui.toasts[0].severity,
        crate::state::ui::ToastSeverity::Warning
    );
    assert!(state.ui.toasts[0].message.contains("project"));
}
