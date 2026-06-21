use super::PlanApprovalPromptState;

#[test]
fn plan_approval_toggles_between_approve_and_deny() {
    let mut o = PlanApprovalPromptState::new(
        "req-1".into(),
        "alice".into(),
        None,
        "# Plan\n- step 1\n- step 2".into(),
    );
    assert!(o.is_approve_focused(), "initial focus should be Approve");
    o.toggle_focus();
    assert!(!o.is_approve_focused());
    o.toggle_focus();
    assert!(o.is_approve_focused());
}

#[test]
fn plan_approval_prompt_gets_awaiting_input_priority() {
    let prompt = crate::state::PanePromptState::PlanApproval(PlanApprovalPromptState::new(
        "req".into(),
        "alice".into(),
        None,
        "".into(),
    ));
    // Priority 2 — same as Question / Elicitation / McpServerApproval.
    // Plan approval blocks the teammate, so it can't be out-prioritized
    // by user-triggered pickers (priority 7+).
    assert_eq!(prompt.priority(), 2);
}

#[test]
fn plan_approval_preserves_from_field_for_response_routing() {
    // The teammate agent name carried in `from` must survive so the
    // UserCommand::PlanApprovalResponse handler in tui_runner knows
    // which inbox to write the response to.
    let o = PlanApprovalPromptState::new(
        "req-42".into(),
        "teammate-delta".into(),
        Some("/plans/delta.md".into()),
        "plan".into(),
    );
    assert_eq!(o.from, "teammate-delta");
    assert_eq!(o.request_id, "req-42");
    assert_eq!(o.plan_file_path.as_deref(), Some("/plans/delta.md"));
}

// === AskUserQuestion footer feedback synthesizers ===
