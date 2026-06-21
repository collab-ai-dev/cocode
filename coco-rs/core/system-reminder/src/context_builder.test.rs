use super::*;
use crate::generator::GeneratorContext;
use coco_config::SystemReminderConfig;
use coco_types::LiveToolPermissionState;
use coco_types::PermissionMode;
use coco_types::TaskListStatus;
use coco_types::TaskRecord;
use coco_types::TodoRecord;
use coco_types::ToolAppState;
use pretty_assertions::assert_eq;

fn cfg() -> SystemReminderConfig {
    SystemReminderConfig::default()
}

// ── apply_app_state ──

#[test]
fn sets_is_plan_mode_from_live_permission_mode() {
    let c = cfg();
    let app = ToolAppState {
        permissions: LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            ..Default::default()
        },
        ..Default::default()
    };

    let ctx = apply_app_state(
        GeneratorContext::builder(&c),
        &app,
        PermissionMode::Default,
        /*is_auto_classifier_active*/ false,
    )
    .build();
    assert!(ctx.is_plan_mode);
    assert!(!ctx.is_auto_mode);
}

#[test]
fn sets_is_auto_mode_from_live_permission_mode() {
    let c = cfg();
    let app = ToolAppState {
        permissions: LiveToolPermissionState {
            mode: Some(PermissionMode::Auto),
            ..Default::default()
        },
        ..Default::default()
    };

    let ctx = apply_app_state(
        GeneratorContext::builder(&c),
        &app,
        PermissionMode::Default,
        /*is_auto_classifier_active*/ false,
    )
    .build();
    assert!(!ctx.is_plan_mode);
    assert!(ctx.is_auto_mode);
}

#[test]
fn plan_mode_with_active_auto_classifier_sets_is_auto_mode() {
    // With the classifier active the auto-mode reminder must still fire
    // even though the live mode is `Plan`.
    let c = cfg();
    let app = ToolAppState {
        permissions: LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            ..Default::default()
        },
        ..Default::default()
    };
    let ctx = apply_app_state(
        GeneratorContext::builder(&c),
        &app,
        PermissionMode::Default,
        /*is_auto_classifier_active*/ true,
    )
    .build();
    assert!(ctx.is_plan_mode, "plan mode is still plan");
    assert!(
        ctx.is_auto_mode,
        "plan + active classifier counts as auto mode"
    );
}

#[test]
fn plan_mode_without_classifier_does_not_set_is_auto_mode() {
    let c = cfg();
    let app = ToolAppState {
        permissions: LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            ..Default::default()
        },
        ..Default::default()
    };
    let ctx = apply_app_state(
        GeneratorContext::builder(&c),
        &app,
        PermissionMode::Default,
        /*is_auto_classifier_active*/ false,
    )
    .build();
    assert!(ctx.is_plan_mode);
    assert!(!ctx.is_auto_mode);
}

#[test]
fn falls_back_to_config_mode_when_app_state_unset() {
    let c = cfg();
    let app = ToolAppState::default();
    let ctx = apply_app_state(
        GeneratorContext::builder(&c),
        &app,
        PermissionMode::Plan,
        /*is_auto_classifier_active*/ false,
    )
    .build();
    assert!(ctx.is_plan_mode);
}

#[test]
fn forwards_plan_exit_and_reentry_flags() {
    let c = cfg();
    let app = ToolAppState {
        has_exited_plan_mode: true,
        needs_plan_mode_exit_attachment: true,
        pending_plan_mode_exit_outcome: Some(coco_types::ExitPlanModeOutcome::NoImplementationPlan),
        needs_auto_mode_exit_attachment: true,
        ..Default::default()
    };
    let ctx = apply_app_state(
        GeneratorContext::builder(&c),
        &app,
        PermissionMode::Default,
        /*is_auto_classifier_active*/ false,
    )
    .build();
    assert!(ctx.is_plan_reentry);
    assert!(ctx.needs_plan_mode_exit_attachment);
    assert_eq!(
        ctx.pending_plan_mode_exit_outcome,
        Some(coco_types::ExitPlanModeOutcome::NoImplementationPlan)
    );
    assert!(ctx.needs_auto_mode_exit_attachment);
}

#[test]
fn copies_plan_tasks_snapshot() {
    let c = cfg();
    let task = TaskRecord {
        id: "t1".to_string(),
        subject: "do it".to_string(),
        description: String::new(),
        active_form: None,
        owner: None,
        status: TaskListStatus::Pending,
        blocks: Vec::new(),
        blocked_by: Vec::new(),
        metadata: Default::default(),
    };
    let app = ToolAppState {
        plan_tasks: vec![task],
        ..Default::default()
    };
    let ctx = apply_app_state(
        GeneratorContext::builder(&c),
        &app,
        PermissionMode::Default,
        /*is_auto_classifier_active*/ false,
    )
    .build();
    assert_eq!(ctx.plan_tasks.len(), 1);
    assert_eq!(ctx.plan_tasks[0].id, "t1");
}

// ── apply_todos_for_key ──

#[test]
fn pulls_todos_for_the_given_key() {
    let c = cfg();
    let mut app = ToolAppState::default();
    let todos = vec![TodoRecord {
        content: "x".to_string(),
        status: "pending".to_string(),
        active_form: "doing x".to_string(),
    }];
    app.todos_by_agent.insert("agent-1".to_string(), todos);

    let ctx = apply_todos_for_key(GeneratorContext::builder(&c), &app, "agent-1").build();
    assert_eq!(ctx.todos.len(), 1);
    assert_eq!(ctx.todos[0].content, "x");
}

#[test]
fn unknown_key_yields_empty_todos() {
    let c = cfg();
    let app = ToolAppState::default();
    let ctx = apply_todos_for_key(GeneratorContext::builder(&c), &app, "missing").build();
    assert!(ctx.todos.is_empty());
}

#[test]
fn apply_helpers_are_composable() {
    let c = cfg();
    let mut todos_map = std::collections::HashMap::new();
    todos_map.insert(
        "k".to_string(),
        vec![TodoRecord {
            content: "c".to_string(),
            status: "pending".to_string(),
            active_form: "a".to_string(),
        }],
    );
    let app = ToolAppState {
        permissions: LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            ..Default::default()
        },
        has_exited_plan_mode: true,
        todos_by_agent: todos_map,
        ..Default::default()
    };

    let ctx = apply_todos_for_key(
        apply_app_state(
            GeneratorContext::builder(&c),
            &app,
            PermissionMode::Default,
            /*is_auto_classifier_active*/ false,
        ),
        &app,
        "k",
    )
    .turn_number(42)
    .build();
    assert!(ctx.is_plan_mode);
    assert!(ctx.is_plan_reentry);
    assert_eq!(ctx.todos.len(), 1);
    assert_eq!(ctx.turn_number, 42);
}
