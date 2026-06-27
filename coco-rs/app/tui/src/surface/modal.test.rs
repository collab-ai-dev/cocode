use super::*;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::PanePromptState;
use crate::state::surface_payloads::PermissionDetail;
use crate::state::surface_payloads::PermissionPromptState;
use crate::state::transcript::TranscriptState;
use crate::surface_content::TextSurfaceContent;
use crate::theme::Theme;
use coco_tui_ui::engine::compatibility::TerminalCompatibility;

fn permission_prompt() -> PermissionPromptState {
    permission_prompt_with_id("p1")
}

fn permission_prompt_with_id(request_id: &str) -> PermissionPromptState {
    PermissionPromptState {
        request_id: request_id.to_string(),
        tool_name: "Bash".to_string(),
        description: "Run command".to_string(),
        detail: PermissionDetail::Generic {
            input_preview: "echo hi".to_string(),
        },
        risk_level: None,
        show_always_allow: false,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: None,
        selected_choice: 0,
        display_input: coco_types::PermissionDisplayInput::Command("echo hi".to_string()),
        original_input: None,
        cwd: None,
        permission_suggestions: vec![],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    }
}

#[test]
fn permission_has_no_fullscreen_surface_and_keeps_history_enabled() {
    assert_eq!(modal_surface_placement(None), None);
    assert!(!history_emission_deferred(None));
}

#[test]
fn permission_state_does_not_defer_history() {
    let now = std::time::Instant::now();
    let mut state = AppState::new();
    state.ui.terminal_focused = true;
    state
        .ui
        .record_surface_interaction(now - std::time::Duration::from_millis(500));
    state
        .ui
        .push_prompt(PanePromptState::Permission(permission_prompt()));

    assert_eq!(modal_surface_placement_for_state(&state, now), None);
    assert!(!history_emission_deferred_for_state(&state, now));
}

#[test]
fn permission_viewport_reserves_prompt_height() {
    let mut state = AppState::new();
    state
        .ui
        .push_prompt(PanePromptState::Permission(permission_prompt()));
    let plan = SurfaceFramePlan {
        modal_placement: None,
        history_surface: HistorySurfaceMode::NativeScrollback,
        attention_requested: false,
    };

    let height =
        crate::surface::viewport::interactive_viewport_desired_height(&state, 80, 24, plan, None);

    assert!(height >= 8, "permission prompt should reserve pane rows");
}

#[test]
fn permission_prompt_uses_substantial_box_size() {
    let state = AppState::new();
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let prompt = permission_prompt();

    let height = required_text_surface_height_for_box(
        TextSurfaceContent::Permission(&prompt),
        &state,
        styles,
        80,
        24,
    );

    assert!(height >= 6);
}

#[test]
fn modal_placement_area_uses_full_area() {
    let modal = ModalState::Help;
    let area = Rect::new(0, 0, 80, 20);
    let input_area = Rect::new(0, 8, 80, 3);

    let placement = modal_placement_area(area, Some(input_area), &modal);

    assert_eq!(placement, area);
}

#[test]
fn permission_never_promotes_to_alt_screen_when_native_viewport_is_short() {
    let now = std::time::Instant::now();
    let mut state = AppState::new();
    state.ui.terminal_focused = true;
    state.ui.record_surface_interaction(now);
    state
        .ui
        .push_prompt(PanePromptState::Permission(permission_prompt()));
    let mut surface = ModalSurfaceState::default();

    let plan = surface.plan_for_native_viewport(
        &state,
        TerminalCompatibility::NativeScrollback,
        now,
        80,
        crate::terminal::NATIVE_VIEWPORT_MAX_HEIGHT,
    );

    assert_eq!(plan.modal_placement, None);
    assert!(!plan.attention_requested);
}

#[test]
fn stale_permission_still_has_no_alt_screen_attention_path() {
    let now = std::time::Instant::now();
    let mut state = AppState::new();
    state.ui.terminal_focused = true;
    state
        .ui
        .record_surface_interaction(now - std::time::Duration::from_secs(3));
    state
        .ui
        .push_prompt(PanePromptState::Permission(permission_prompt()));

    assert_eq!(modal_surface_placement_for_state(&state, now), None);
}

#[test]
fn unfocused_permission_still_has_no_alt_screen_attention_path() {
    let now = std::time::Instant::now();
    let mut state = AppState::new();
    state.ui.terminal_focused = false;
    state.ui.record_surface_interaction(now);
    state
        .ui
        .push_prompt(PanePromptState::Permission(permission_prompt()));

    assert_eq!(modal_surface_placement_for_state(&state, now), None);
}

#[test]
fn cost_warning_is_a_pane_prompt_not_alt_screen() {
    let now = std::time::Instant::now();
    let mut state = AppState::new();
    state.ui.push_prompt(PanePromptState::CostWarning(
        crate::state::CostWarningPromptState {
            current_cost_cents: 1200,
            threshold_cents: 1000,
        },
    ));

    assert_eq!(modal_surface_placement_for_state(&state, now), None);
}

#[test]
fn modal_surface_state_latches_no_prompt_surface() {
    let now = std::time::Instant::now();
    let mut state = AppState::new();
    state.ui.terminal_focused = true;
    state.ui.record_surface_interaction(now);
    state
        .ui
        .push_prompt(PanePromptState::Permission(permission_prompt()));
    let mut surface = ModalSurfaceState::default();

    let first = surface.plan(&state, TerminalCompatibility::NativeScrollback, now);
    let second = surface.plan(
        &state,
        TerminalCompatibility::NativeScrollback,
        now + std::time::Duration::from_secs(10),
    );

    assert_eq!(first.modal_placement, None);
    assert_eq!(second.modal_placement, None);
    assert!(!second.attention_requested);
}

#[test]
fn prompt_surface_never_requests_attention_alt_screen_latch() {
    let now = std::time::Instant::now();
    let mut state = AppState::new();
    state.ui.terminal_focused = false;
    state.ui.record_surface_interaction(now);
    state
        .ui
        .push_prompt(PanePromptState::Permission(permission_prompt()));
    let mut surface = ModalSurfaceState::default();

    let first = surface.plan(&state, TerminalCompatibility::NativeScrollback, now);
    let second = surface.plan(
        &state,
        TerminalCompatibility::NativeScrollback,
        now + std::time::Duration::from_millis(100),
    );

    assert_eq!(first.modal_placement, None);
    assert!(!first.attention_requested);
    assert_eq!(second.modal_placement, None);
    assert!(!second.attention_requested);
}

#[test]
fn turn_completion_timestamp_does_not_affect_prompt_surface() {
    let now = std::time::Instant::now();
    let mut state = AppState::new();
    state.ui.terminal_focused = true;
    state.session.last_user_interaction_at = now;
    state
        .ui
        .push_prompt(PanePromptState::Permission(permission_prompt()));

    assert_eq!(modal_surface_placement_for_state(&state, now), None);
}

#[test]
fn queued_same_kind_prompt_keeps_native_surface() {
    let now = std::time::Instant::now();
    let mut state = AppState::new();
    state.ui.terminal_focused = true;
    state.ui.record_surface_interaction(now);
    state
        .ui
        .push_prompt(PanePromptState::Permission(permission_prompt_with_id("p1")));
    state
        .ui
        .push_prompt(PanePromptState::Permission(permission_prompt_with_id("p2")));
    let mut surface = ModalSurfaceState::default();

    let first = surface.plan(&state, TerminalCompatibility::NativeScrollback, now);
    state.ui.dismiss_prompt();
    state
        .ui
        .record_surface_interaction(now - std::time::Duration::from_secs(10));
    let second = surface.plan(
        &state,
        TerminalCompatibility::NativeScrollback,
        now + std::time::Duration::from_secs(10),
    );

    assert_eq!(first.modal_placement, None);
    assert_eq!(second.modal_placement, None);
    assert!(!second.attention_requested);
}

#[test]
fn transcript_uses_alt_screen_placement_and_defers_history() {
    let modal = ModalState::Transcript(TranscriptState::new());

    assert_eq!(
        modal_surface_placement(Some(&modal)),
        Some(ModalSurfacePlacement::AltScreen)
    );
    assert!(history_emission_deferred(Some(&modal)));
}
