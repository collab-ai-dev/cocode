use tokio::sync::mpsc;

use super::cycle_effort;
use super::filtered_models;
use crate::command::UserCommand;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::ModelEntry;
use crate::state::ModelPickerState;
use crate::state::ProviderUnavailableReason;
use crate::state::ui::ToastSeverity;
use coco_types::ModelRole;
use coco_types::ReasoningEffort;

fn picker(entries: Vec<ModelEntry>, selected: i32, effort: Option<ReasoningEffort>) -> AppState {
    let mut s = AppState::new();
    s.ui.show_modal(ModalState::ModelPicker(ModelPickerState {
        role: ModelRole::Main,
        entries,
        filter: String::new(),
        selected,
        effort,
    }));
    s
}

fn entry(
    model_id: &str,
    efforts: &[ReasoningEffort],
    default: Option<ReasoningEffort>,
) -> ModelEntry {
    ModelEntry {
        provider: "test".into(),
        provider_display: "Test".into(),
        model_id: model_id.into(),
        display_name: model_id.into(),
        context_window: Some(200_000),
        supported_efforts: efforts.to_vec(),
        default_effort: default,
        is_current_for_role: false,
        unavailable_reasons: Vec::new(),
    }
}

#[test]
fn cycle_effort_advances_through_supported_levels() {
    let levels = [
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
    ];
    let mut s = picker(
        vec![entry("m", &levels, Some(ReasoningEffort::Low))],
        0,
        Some(ReasoningEffort::Low),
    );
    cycle_effort(&mut s, 1);
    let Some(ModalState::ModelPicker(m)) = s.ui.modal.as_ref() else {
        panic!()
    };
    assert_eq!(m.effort, Some(ReasoningEffort::Medium));
    cycle_effort(&mut s, 1);
    let Some(ModalState::ModelPicker(m)) = s.ui.modal.as_ref() else {
        panic!()
    };
    assert_eq!(m.effort, Some(ReasoningEffort::High));
}

#[test]
fn cycle_effort_wraps_around_at_endpoints() {
    let levels = [
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
    ];
    let mut s = picker(
        vec![entry("m", &levels, Some(ReasoningEffort::High))],
        0,
        Some(ReasoningEffort::High),
    );
    cycle_effort(&mut s, 1);
    let Some(ModalState::ModelPicker(m)) = s.ui.modal.as_ref() else {
        panic!()
    };
    assert_eq!(m.effort, Some(ReasoningEffort::Low));
    cycle_effort(&mut s, -1);
    let Some(ModalState::ModelPicker(m)) = s.ui.modal.as_ref() else {
        panic!()
    };
    assert_eq!(m.effort, Some(ReasoningEffort::High));
}

#[test]
fn cycle_effort_noops_when_no_supported_levels() {
    let mut s = picker(vec![entry("m", &[], None)], 0, None);
    cycle_effort(&mut s, 1);
    let Some(ModalState::ModelPicker(m)) = s.ui.modal.as_ref() else {
        panic!()
    };
    assert!(m.effort.is_none());
}

#[test]
fn cycle_effort_noops_outside_picker() {
    let mut s = AppState::new();
    cycle_effort(&mut s, 1);
    assert!(!s.ui.has_active_surface());
}

#[tokio::test]
async fn confirm_model_picker_blocks_unavailable_provider() {
    let mut unavailable = entry("m", &[], None);
    unavailable
        .unavailable_reasons
        .push(ProviderUnavailableReason::MissingApiKey {
            env_key: "TEST_API_KEY".to_string(),
        });
    let mut s = picker(vec![unavailable], 0, None);
    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);

    crate::modal_pane::route_confirm(&mut s, &tx).await;

    assert!(rx.try_recv().is_err(), "no model-change command sent");
    assert!(matches!(
        s.ui.modal.as_ref(),
        Some(ModalState::ModelPicker(_))
    ));
    assert_eq!(s.ui.toasts.len(), 1);
    assert_eq!(s.ui.toasts[0].severity, ToastSeverity::Warning);
    assert!(s.ui.toasts[0].message.contains("TEST_API_KEY"));
}

#[test]
fn filtered_models_matches_provider_display() {
    let entries = vec![
        ModelEntry {
            provider: "anthropic".into(),
            provider_display: "Anthropic".into(),
            model_id: "claude-haiku-4-5".into(),
            display_name: "Claude Haiku".into(),
            context_window: Some(200_000),
            supported_efforts: vec![],
            default_effort: None,
            is_current_for_role: false,
            unavailable_reasons: Vec::new(),
        },
        ModelEntry {
            provider: "openai".into(),
            provider_display: "OpenAI".into(),
            model_id: "gpt-5-4".into(),
            display_name: "GPT-5.4".into(),
            context_window: Some(272_000),
            supported_efforts: vec![],
            default_effort: None,
            is_current_for_role: false,
            unavailable_reasons: Vec::new(),
        },
    ];
    let m = ModelPickerState {
        role: ModelRole::Main,
        entries,
        filter: "open".to_string(),
        selected: 0,
        effort: None,
    };
    let filtered = filtered_models(&m);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].provider, "openai");
}

#[tokio::test]
async fn confirm_emits_set_model_role_and_updates_main_model() {
    // G6: the TUI→engine half of the model-picker round-trip. Selecting an
    // available model must emit `SetModelRole` (so the engine actually
    // rebinds the role) AND optimistically update the status-bar model for
    // Main. Previously untested — a regression here means the picker UI
    // updates but the next turn keeps using the old model.
    let (tx, mut rx) = mpsc::channel::<UserCommand>(4);
    let mut s = AppState::new();
    let m = ModelPickerState {
        role: ModelRole::Main,
        entries: vec![entry(
            "claude-opus-4-8",
            &[ReasoningEffort::High],
            Some(ReasoningEffort::High),
        )],
        filter: String::new(),
        selected: 0,
        effort: Some(ReasoningEffort::High),
    };
    super::confirm(&mut s, m, &tx).await;

    match rx.try_recv() {
        Ok(UserCommand::SetModelRole {
            role,
            provider,
            model_id,
            effort,
        }) => {
            assert_eq!(role, ModelRole::Main);
            assert_eq!(provider, "test");
            assert_eq!(model_id, "claude-opus-4-8");
            assert_eq!(effort, Some(ReasoningEffort::High));
        }
        other => panic!("expected SetModelRole on the wire, got {other:?}"),
    }
    assert_eq!(
        s.session.model, "claude-opus-4-8",
        "Main selection should optimistically update the status-bar model"
    );
    // (The unavailable/blocked branch is covered by
    // `confirm_model_picker_blocks_unavailable_provider`.)
}
