use super::*;
use crate::state::PermissionPromptState;

#[test]
fn prompt_text_surface_skips_exit_plan_mode_permission() {
    let prompt = PanePromptState::Permission(PermissionPromptState {
        request_id: "req-1".into(),
        tool_name: coco_types::ToolName::ExitPlanMode.as_str().into(),
        description: "Exit plan mode?".into(),
        detail: PermissionDetail::ExitPlanMode {
            outcome: coco_types::ExitPlanModeOutcome::ImplementationPlan,
            plan: Some("- step".into()),
            plan_file_path: Some("/tmp/plan.md".into()),
            allowed_prompts: vec![],
        },
        risk_level: None,
        show_always_allow: false,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: None,
        selected_choice: 0,
        display_input: coco_types::PermissionDisplayInput::Empty,
        original_input: None,
        cwd: None,
        permission_suggestions: vec![],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    });

    assert!(prompt_text_surface(&prompt).is_none());
}
