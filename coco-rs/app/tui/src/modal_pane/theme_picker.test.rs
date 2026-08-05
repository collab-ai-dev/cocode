use super::*;

use crate::state::ThemePickerOrigin;
use crate::theme::ThemeChoice;
use crate::theme::ThemeConfig;
use crate::theme::ThemeRuntimeState;
use crate::theme::ThemeSetting;
use crate::widgets::settings_panel::SettingsPanelState;

fn channel() -> (mpsc::Sender<UserCommand>, mpsc::Receiver<UserCommand>) {
    mpsc::channel(8)
}

fn different_choice(state: &AppState) -> ThemeChoice {
    state
        .ui
        .theme_state
        .choices
        .iter()
        .find(|choice| choice.setting != state.ui.theme_state.setting)
        .cloned()
        .expect("default registry has an alternative theme")
}

fn picker(state: &AppState, origin: ThemePickerOrigin) -> ThemePickerState {
    let choice = different_choice(state);
    ThemePickerState {
        selected: state
            .ui
            .theme_state
            .choices
            .iter()
            .position(|candidate| candidate == &choice)
            .expect("alternative theme is in the live registry") as i32,
        original_setting: state.ui.theme_state.setting.clone(),
        origin,
    }
}

#[tokio::test]
async fn persistence_failure_rolls_back_and_keeps_picker_open() {
    let mut state = AppState::new();
    let original = state.ui.theme_state.setting.clone();
    let picker = picker(&state, ThemePickerOrigin::Standalone);
    let (tx, _rx) = channel();

    confirm_with(&mut state, picker, &tx, |_| anyhow::bail!("read only")).await;

    assert_eq!(state.ui.theme_state.setting, original);
    assert!(matches!(state.ui.modal, Some(ModalState::ThemePicker(_))));
    assert_eq!(state.ui.toasts.len(), 1);
}

#[tokio::test]
async fn settings_origin_commit_restores_exact_parent_state() {
    let mut state = AppState::new();
    let parent = SettingsPanelState {
        filter: "theme".to_string(),
        selected: 0,
    };
    let picker = picker(&state, ThemePickerOrigin::Settings(Box::new(parent)));
    state.ui.show_modal(ModalState::ThemePicker(picker.clone()));
    let ModalState::ThemePicker(picker) = state.ui.take_modal().expect("active picker") else {
        panic!("expected theme picker")
    };
    let (tx, mut rx) = channel();

    confirm_with(&mut state, picker, &tx, |_| Ok(PathBuf::from("theme.json"))).await;

    let Some(ModalState::Settings(parent)) = state.ui.modal.as_ref() else {
        panic!("settings parent should be restored")
    };
    assert_eq!(parent.filter, "theme");
    assert_eq!(parent.selected, 0);
    assert!(
        rx.try_recv().is_err(),
        "nested commit is not a /theme command"
    );
}

#[tokio::test]
async fn settings_origin_cancel_restores_parent_without_theme_transcript() {
    let mut state = AppState::new();
    let parent = SettingsPanelState::default();
    let picker = picker(&state, ThemePickerOrigin::Settings(Box::new(parent)));
    state.ui.show_modal(ModalState::ThemePicker(picker));
    let (tx, mut rx) = channel();

    crate::modal_pane::close_modal_with_feedback(&mut state, &tx).await;

    assert!(matches!(state.ui.modal, Some(ModalState::Settings(_))));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn settings_parent_survives_higher_priority_modal_preemption() {
    let mut state = AppState::new();
    let parent = SettingsPanelState {
        filter: "theme".to_string(),
        ..Default::default()
    };
    let picker = picker(&state, ThemePickerOrigin::Settings(Box::new(parent)));
    state.ui.show_modal(ModalState::ThemePicker(picker));
    state
        .ui
        .show_modal(ModalState::Error("interruption".to_string()));
    assert!(matches!(state.ui.modal, Some(ModalState::Error(_))));
    state.ui.dismiss_modal();
    assert!(matches!(state.ui.modal, Some(ModalState::ThemePicker(_))));
    let (tx, _rx) = channel();

    crate::modal_pane::close_modal_with_feedback(&mut state, &tx).await;

    let Some(ModalState::Settings(parent)) = state.ui.modal.as_ref() else {
        panic!("settings parent should survive preemption")
    };
    assert_eq!(parent.filter, "theme");
}

#[tokio::test]
async fn cancel_after_external_reload_preserves_reloaded_theme() {
    let mut state = AppState::new();
    let picker = picker(&state, ThemePickerOrigin::Standalone);
    state.ui.show_modal(ModalState::ThemePicker(picker));
    let reloaded = ThemeRuntimeState::from_config(
        PathBuf::from("theme.json"),
        ThemeConfig {
            active: ThemeSetting::Named("light".to_string()),
            ..ThemeConfig::default()
        },
    )
    .expect("light theme runtime");
    state.ui.apply_theme_reload(reloaded.clone());
    let (tx, _rx) = channel();

    crate::modal_pane::close_modal_with_feedback(&mut state, &tx).await;

    assert!(state.ui.modal.is_none());
    assert_eq!(state.ui.theme_state.setting, reloaded.setting);
    assert_eq!(state.ui.theme_state.active_id, "light");
}
