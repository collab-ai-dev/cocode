use super::*;

fn empty_state() -> tokio::sync::RwLock<coco_types::ToolAppState> {
    tokio::sync::RwLock::new(coco_types::ToolAppState::default())
}

fn goal_hook_count(registry: &coco_hooks::HookRegistry) -> usize {
    // `remove_matching_hooks` returns the removed defs; re-register them so the
    // count read is non-destructive for the rest of the test.
    let removed = registry.remove_matching_hooks(is_managed_goal_hook);
    let count = removed.len();
    for hook in removed {
        registry.register(hook);
    }
    count
}

#[test]
fn active_goal_status_matches_noninteractive_contract() {
    let mut goal = coco_types::ActiveGoal {
        condition: "finish migration".to_string(),
        iterations: 0,
        set_at_ms: 0,
        tokens_at_start: 0,
        last_reason: None,
    };
    assert_eq!(
        format_active_goal_status(&goal),
        "Goal active: finish migration (not yet evaluated)"
    );

    goal.iterations = 2;
    goal.last_reason = Some(" tests still failing\n rerun needed ".to_string());
    assert_eq!(
        format_active_goal_status(&goal),
        "Goal active: finish migration (2 turns)\nLast check: tests still failing rerun needed"
    );
}

#[test]
fn find_last_achieved_goal_skips_clear_sentinel() {
    let clear = coco_messages::Message::Attachment(
        coco_messages::AttachmentMessage::silent_goal_status(coco_types::GoalStatusPayload {
            met: true,
            condition: "cleared".to_string(),
            sentinel: true,
            ..Default::default()
        }),
    );
    let achieved = coco_messages::Message::Attachment(
        coco_messages::AttachmentMessage::silent_goal_status(coco_types::GoalStatusPayload {
            met: true,
            condition: "done".to_string(),
            iterations: Some(3),
            sentinel: false,
            ..Default::default()
        }),
    );
    let history = vec![Arc::new(achieved), Arc::new(clear)];

    let found = find_last_achieved_goal(&history).expect("achieved goal");

    assert_eq!(found.condition, "done");
    assert_eq!(found.iterations, Some(3));
}

#[test]
fn find_restorable_goal_condition_uses_latest_goal_status() {
    let unmet = coco_messages::Message::Attachment(
        coco_messages::AttachmentMessage::silent_goal_status(coco_types::GoalStatusPayload {
            met: false,
            condition: "finish tests".to_string(),
            sentinel: false,
            ..Default::default()
        }),
    );
    let achieved = coco_messages::Message::Attachment(
        coco_messages::AttachmentMessage::silent_goal_status(coco_types::GoalStatusPayload {
            met: true,
            condition: "finish tests".to_string(),
            sentinel: false,
            ..Default::default()
        }),
    );

    assert_eq!(
        find_restorable_goal_condition(&[Arc::new(unmet.clone())]).as_deref(),
        Some("finish tests")
    );
    assert_eq!(
        find_restorable_goal_condition(&[Arc::new(unmet), Arc::new(achieved)]),
        None
    );
}

#[test]
fn find_restorable_goal_condition_treats_clear_sentinel_as_terminal() {
    let unmet = coco_messages::Message::Attachment(
        coco_messages::AttachmentMessage::silent_goal_status(coco_types::GoalStatusPayload {
            met: false,
            condition: "finish tests".to_string(),
            sentinel: false,
            ..Default::default()
        }),
    );
    let clear = coco_messages::Message::Attachment(
        coco_messages::AttachmentMessage::silent_goal_status(coco_types::GoalStatusPayload {
            met: true,
            condition: "finish tests".to_string(),
            sentinel: true,
            ..Default::default()
        }),
    );

    assert_eq!(
        find_restorable_goal_condition(&[Arc::new(unmet), Arc::new(clear)]),
        None
    );
}

#[test]
fn goal_hook_matcher_requires_managed_session_stop_prompt() {
    let mut hook = managed_goal_hook("done".to_string());

    assert!(is_managed_goal_hook(&hook));
    hook.managed_by = None;
    assert!(!is_managed_goal_hook(&hook));
    hook.managed_by = Some(coco_hooks::ManagedHookKind::Goal);
    hook.scope = coco_types::HookScope::User;
    assert!(!is_managed_goal_hook(&hook));
}

#[test]
fn goal_display_args_matches_request_variant() {
    assert_eq!(
        goal_display_args(&coco_commands::GoalCommandRequest::Status),
        ""
    );
    assert_eq!(
        goal_display_args(&coco_commands::GoalCommandRequest::Clear),
        "clear"
    );
    assert_eq!(
        goal_display_args(&coco_commands::GoalCommandRequest::Set {
            condition: "ship it".to_string(),
        }),
        "ship it"
    );
}

#[tokio::test]
async fn resolve_status_with_no_goal_returns_usage() {
    let state = empty_state();
    let registry = coco_hooks::HookRegistry::new();

    let outcome = resolve_goal_request(
        coco_commands::GoalCommandRequest::Status,
        &state,
        &registry,
        &[],
        0,
        GoalGate::default(),
    )
    .await;

    assert_eq!(
        outcome,
        GoalOutcome::Text("No goal set. Usage: `/goal <condition>`".to_string())
    );
}

#[tokio::test]
async fn resolve_status_with_active_goal_formats_active() {
    let state = empty_state();
    state.write().await.active_goal = Some(active_goal("finish migration".to_string(), 0));
    let registry = coco_hooks::HookRegistry::new();

    let outcome = resolve_goal_request(
        coco_commands::GoalCommandRequest::Status,
        &state,
        &registry,
        &[],
        0,
        GoalGate::default(),
    )
    .await;

    assert_eq!(
        outcome,
        GoalOutcome::Text("Goal active: finish migration (not yet evaluated)".to_string())
    );
}

#[tokio::test]
async fn resolve_set_registers_hook_seeds_state_and_kicks_off() {
    let state = empty_state();
    let registry = coco_hooks::HookRegistry::new();

    let outcome = resolve_goal_request(
        coco_commands::GoalCommandRequest::Set {
            condition: "all tests pass".to_string(),
        },
        &state,
        &registry,
        &[],
        /*tokens_at_start*/ 42,
        GoalGate::default(),
    )
    .await;

    assert_eq!(
        outcome,
        GoalOutcome::SetAndRun {
            status: goal_status_sentinel(false, "all tests pass".to_string()),
            text: "Goal set: all tests pass".to_string(),
            kickoff: build_goal_kickoff_prompt("all tests pass"),
        }
    );

    let active = state.read().await.active_goal.clone().expect("active goal");
    assert_eq!(active.condition, "all tests pass");
    assert_eq!(active.tokens_at_start, 42);
    assert_eq!(active.iterations, 0);
    assert_eq!(goal_hook_count(&registry), 1);
}

#[tokio::test]
async fn restore_goal_from_history_reinstalls_managed_hook() {
    let state = empty_state();
    let registry = coco_hooks::HookRegistry::new();
    let history = vec![Arc::new(coco_messages::Message::Attachment(
        coco_messages::AttachmentMessage::silent_goal_status(coco_types::GoalStatusPayload {
            met: false,
            condition: "ship feature".to_string(),
            ..Default::default()
        }),
    ))];

    let restored = restore_goal_from_history(&history, &state, &registry, 77, GoalGate::default())
        .await
        .expect("restored goal");

    assert_eq!(restored.condition, "ship feature");
    assert_eq!(restored.tokens_at_start, 77);
    assert_eq!(state.read().await.active_goal.as_ref(), Some(&restored));
    assert_eq!(goal_hook_count(&registry), 1);
}

#[tokio::test]
async fn resolve_set_replaces_an_existing_goal_hook() {
    let state = empty_state();
    let registry = coco_hooks::HookRegistry::new();
    registry.register(managed_goal_hook("old goal".to_string()));

    resolve_goal_request(
        coco_commands::GoalCommandRequest::Set {
            condition: "new goal".to_string(),
        },
        &state,
        &registry,
        &[],
        0,
        GoalGate::default(),
    )
    .await;

    // Exactly one goal hook remains, and it carries the new condition.
    let removed = registry.remove_matching_hooks(is_managed_goal_hook);
    assert_eq!(removed.len(), 1);
    assert_eq!(prompt_from_hook(&removed[0]).as_deref(), Some("new goal"));
}

#[tokio::test]
async fn resolve_set_hooks_restricted_returns_gate_and_mutates_nothing() {
    let state = empty_state();
    let registry = coco_hooks::HookRegistry::new();

    let outcome = resolve_goal_request(
        coco_commands::GoalCommandRequest::Set {
            condition: "blocked".to_string(),
        },
        &state,
        &registry,
        &[],
        0,
        GoalGate {
            hooks_restricted: true,
            trust_rejected: false,
        },
    )
    .await;

    assert_eq!(outcome, GoalOutcome::Text(HOOKS_GATE_MESSAGE.to_string()));
    assert!(state.read().await.active_goal.is_none());
    assert_eq!(goal_hook_count(&registry), 0);
}

#[tokio::test]
async fn resolve_set_trust_rejected_returns_trust_gate() {
    let state = empty_state();
    let registry = coco_hooks::HookRegistry::new();

    let outcome = resolve_goal_request(
        coco_commands::GoalCommandRequest::Set {
            condition: "blocked".to_string(),
        },
        &state,
        &registry,
        &[],
        0,
        GoalGate {
            hooks_restricted: false,
            trust_rejected: true,
        },
    )
    .await;

    assert_eq!(outcome, GoalOutcome::Text(TRUST_GATE_MESSAGE.to_string()));
    assert!(state.read().await.active_goal.is_none());
}

#[tokio::test]
async fn resolve_set_hooks_gate_precedes_trust_gate() {
    let state = empty_state();
    let registry = coco_hooks::HookRegistry::new();

    let outcome = resolve_goal_request(
        coco_commands::GoalCommandRequest::Set {
            condition: "blocked".to_string(),
        },
        &state,
        &registry,
        &[],
        0,
        GoalGate {
            hooks_restricted: true,
            trust_rejected: true,
        },
    )
    .await;

    // Both gates closed → the hooks message wins (structural unavailability).
    assert_eq!(outcome, GoalOutcome::Text(HOOKS_GATE_MESSAGE.to_string()));
}

#[tokio::test]
async fn resolve_clear_active_goal_clears_state_and_hook() {
    let state = empty_state();
    let registry = coco_hooks::HookRegistry::new();
    // Seed an active goal exactly as a prior `set` would have.
    resolve_goal_request(
        coco_commands::GoalCommandRequest::Set {
            condition: "finish it".to_string(),
        },
        &state,
        &registry,
        &[],
        0,
        GoalGate::default(),
    )
    .await;

    let outcome = resolve_goal_request(
        coco_commands::GoalCommandRequest::Clear,
        &state,
        &registry,
        &[],
        0,
        GoalGate::default(),
    )
    .await;

    assert_eq!(
        outcome,
        GoalOutcome::StatusThenText {
            status: goal_status_sentinel(true, "finish it".to_string()),
            text: "Goal cleared: finish it".to_string(),
        }
    );
    assert!(state.read().await.active_goal.is_none());
    assert_eq!(goal_hook_count(&registry), 0);
}

#[tokio::test]
async fn resolve_clear_without_goal_reports_none() {
    let state = empty_state();
    let registry = coco_hooks::HookRegistry::new();

    let outcome = resolve_goal_request(
        coco_commands::GoalCommandRequest::Clear,
        &state,
        &registry,
        &[],
        0,
        GoalGate::default(),
    )
    .await;

    assert_eq!(outcome, GoalOutcome::Text("No goal set".to_string()));
}
