use super::PanePromptState;
use crate::state::PermissionDetail;
use crate::state::PermissionPromptState;
use crate::state::surface_payloads::PlanEntryPromptState;
use crate::state::surface_payloads::SandboxPermissionPromptState;

fn permission_prompt() -> PermissionPromptState {
    PermissionPromptState {
        request_id: "permission-1".into(),
        tool_name: "Bash".into(),
        description: "Run command".into(),
        detail: PermissionDetail::Generic {
            input_preview: "echo hi".into(),
        },
        risk_level: None,
        show_always_allow: false,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: None,
        selected_choice: 0,
        display_input: coco_types::PermissionDisplayInput::Command("echo hi".into()),
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
fn permission_prompt_pauses_clock() {
    let prompt = PanePromptState::Permission(permission_prompt());
    assert!(prompt.pauses_status_clock());
}

#[test]
fn sandbox_permission_prompt_pauses_clock() {
    // DIVERGE: no SandboxPermission analog upstream; we still pause
    // because it shares the "tool blocked on user approval" semantic.
    let prompt = PanePromptState::SandboxPermission(SandboxPermissionPromptState {
        request_id: "sandbox-1".into(),
        description: "Sandbox access requested".into(),
    });
    assert!(prompt.pauses_status_clock());
}

#[test]
fn plan_entry_prompt_does_not_pause_clock() {
    // Only tool-permission pauses the elapsed clock. Plan-entry is a
    // different dialog focus; the elapsed clock keeps ticking through it.
    let prompt = PanePromptState::PlanEntry(PlanEntryPromptState {
        description: "Entering plan mode".into(),
    });
    assert!(!prompt.pauses_status_clock());
}
