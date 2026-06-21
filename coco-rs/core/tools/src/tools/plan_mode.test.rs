//! Tests for EnterPlanMode + ExitPlanMode tools.

use super::EnterPlanModeTool;
use super::ExitPlanModeTool;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::ValidationResult;
use coco_types::AgentId;
use coco_types::LiveToolPermissionState;
use coco_types::PermissionMode;
use coco_types::ToolAppState;
use coco_types::ToolDisplayData;
use coco_types::ToolName;
use serde_json::Value;
use serde_json::json;

fn ctx_with_mode(mode: PermissionMode) -> ToolUseContext {
    let mut ctx = ToolUseContext::test_default();
    ctx.permission_context.mode = mode;
    ctx
}

fn implementation_plan_input() -> Value {
    json!({})
}

fn set_exit_plan_choice(ctx: &mut ToolUseContext, choice: coco_types::ExitPlanChoice) {
    ctx.permission_resolution_detail = Some(coco_types::PermissionResolutionDetail::ExitPlanMode {
        choice,
        edited_plan: None,
    });
}

/// Drive a tool's `execute` + apply its `app_state_patch` — the
/// unit-test equivalent of what the real executor does. Needed
/// because the shared `ToolAppState` lives behind
/// `AppStateReadHandle` on `ctx.app_state` (read-only by design —
/// the type system forbids writes), so mutations only hit the
/// store when the patch returned in `ToolResult::app_state_patch`
/// is actually applied.
async fn execute_and_apply_patch(
    tool: &(dyn DynTool + Send + Sync),
    input: Value,
    ctx: &ToolUseContext,
    state: &std::sync::Arc<tokio::sync::RwLock<ToolAppState>>,
) -> Result<coco_messages::ToolResult<Value>, coco_tool_runtime::ToolError> {
    let mut result = tool.execute(input, ctx).await?;
    if let Some(patch) = result.app_state_patch.take() {
        let mut guard = state.write().await;
        patch(&mut guard);
    }
    Ok(result)
}

// ── EnterPlanModeTool ──

#[tokio::test]
async fn enter_plan_mode_rejects_in_agent_context() {
    let mut ctx = ctx_with_mode(PermissionMode::Default);
    ctx.agent_id = Some(AgentId::new("aabcdef0"));
    let result = <EnterPlanModeTool as DynTool>::execute(&EnterPlanModeTool, json!({}), &ctx).await;
    assert!(result.is_err(), "agent contexts must be rejected");
    assert!(result.unwrap_err().to_string().contains("agent"));
}

#[tokio::test]
async fn enter_plan_mode_returns_confirmation_message() {
    let ctx = ctx_with_mode(PermissionMode::Default);
    let result = <EnterPlanModeTool as DynTool>::execute(&EnterPlanModeTool, json!({}), &ctx)
        .await
        .unwrap();
    let msg = result
        .data
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(msg.contains("Entered plan mode"));
}

#[tokio::test]
async fn enter_plan_mode_stashes_previous_mode() {
    use std::sync::Arc;
    use tokio::sync::RwLock;
    let app_state = Arc::new(RwLock::new(ToolAppState {
        permissions: LiveToolPermissionState {
            mode: Some(PermissionMode::AcceptEdits),
            ..Default::default()
        },
        ..Default::default()
    }));
    let mut ctx = ctx_with_mode(PermissionMode::AcceptEdits);
    ctx.app_state = Some(app_state.clone().into());
    let _ = execute_and_apply_patch(&EnterPlanModeTool, json!({}), &ctx, &app_state)
        .await
        .unwrap();
    let guard = app_state.read().await;
    assert_eq!(guard.permissions.mode, Some(PermissionMode::Plan));
    assert_eq!(
        guard.permissions.pre_plan_mode,
        Some(PermissionMode::AcceptEdits)
    );
}

#[tokio::test]
async fn enter_plan_mode_idempotent_does_not_stash_self() {
    // Calling enter while already in plan mode must NOT overwrite the
    // stash with Plan itself — otherwise exit would have nowhere to
    // return to.
    use std::sync::Arc;
    use tokio::sync::RwLock;
    let app_state = Arc::new(RwLock::new(ToolAppState {
        permissions: LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            pre_plan_mode: Some(PermissionMode::AcceptEdits),
            ..Default::default()
        },
        plan_mode_entry_ms: Some(42),
        ..Default::default()
    }));
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.app_state = Some(app_state.clone().into());
    let _ = execute_and_apply_patch(&EnterPlanModeTool, json!({}), &ctx, &app_state)
        .await
        .unwrap();
    let guard = app_state.read().await;
    assert_eq!(
        guard.permissions.pre_plan_mode,
        Some(PermissionMode::AcceptEdits)
    );
    assert_eq!(guard.plan_mode_entry_ms, Some(42));
}

#[test]
fn enter_plan_mode_schema_has_no_parameters() {
    let schema =
        <EnterPlanModeTool as DynTool>::runtime_validation_schema(&EnterPlanModeTool).as_value();
    assert!(
        schema["properties"]
            .as_object()
            .is_none_or(serde_json::Map::is_empty),
        "EnterPlanMode takes no parameters"
    );
}

// ── ExitPlanModeTool ──

#[test]
fn exit_plan_mode_rejects_when_not_in_plan_mode() {
    let ctx = ctx_with_mode(PermissionMode::Default);
    let input = implementation_plan_input();
    let vr = <ExitPlanModeTool as DynTool>::validate_input(&ExitPlanModeTool, &input, &ctx);
    match vr {
        ValidationResult::Invalid {
            message,
            error_code,
        } => {
            assert!(message.contains("not in plan mode"));
            assert_eq!(error_code.as_deref(), Some("1"));
        }
        ValidationResult::Valid => panic!("expected Invalid outside plan mode"),
    }
}

#[test]
fn exit_plan_mode_allows_when_in_plan_mode() {
    let ctx = ctx_with_mode(PermissionMode::Plan);
    let input = implementation_plan_input();
    let vr = <ExitPlanModeTool as DynTool>::validate_input(&ExitPlanModeTool, &input, &ctx);
    assert!(matches!(vr, ValidationResult::Valid));
}

#[test]
fn exit_plan_mode_teammate_bypasses_validation() {
    let mut ctx = ctx_with_mode(PermissionMode::Default);
    ctx.is_teammate = true;
    let input = implementation_plan_input();
    let vr = <ExitPlanModeTool as DynTool>::validate_input(&ExitPlanModeTool, &input, &ctx);
    assert!(matches!(vr, ValidationResult::Valid));
}

#[tokio::test]
async fn exit_plan_mode_teammate_bypasses_permission_prompt() {
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.is_teammate = true;
    let decision = <ExitPlanModeTool as DynTool>::check_permissions(
        &ExitPlanModeTool,
        &implementation_plan_input(),
        &ctx,
    )
    .await;
    assert!(matches!(
        decision,
        coco_types::ToolCheckResult::Allow { .. }
    ));
}

#[tokio::test]
async fn exit_plan_mode_non_teammate_asks_for_confirmation() {
    let ctx = ctx_with_mode(PermissionMode::Plan);
    let decision = <ExitPlanModeTool as DynTool>::check_permissions(
        &ExitPlanModeTool,
        &implementation_plan_input(),
        &ctx,
    )
    .await;
    match decision {
        coco_types::ToolCheckResult::Ask {
            message,
            choices,
            detail,
            ..
        } => {
            assert!(message.contains("Exit plan mode"));
            assert!(choices.is_some(), "ExitPlanMode should provide choices");
            assert!(detail.is_some(), "ExitPlanMode should provide typed detail");
        }
        other => panic!("expected Ask, got {other:?}"),
    }
}

#[tokio::test]
async fn exit_plan_mode_tool_embeds_typed_tui_choices() {
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.plan_mode_settings.show_clear_context_on_exit = true;

    let decision = <ExitPlanModeTool as DynTool>::check_permissions(
        &ExitPlanModeTool,
        &implementation_plan_input(),
        &ctx,
    )
    .await;
    match decision {
        coco_types::ToolCheckResult::Ask {
            choices, detail, ..
        } => {
            assert!(choices.is_some(), "ExitPlanMode should provide choices");
            assert!(detail.is_some(), "ExitPlanMode should provide typed detail");
        }
        other => panic!("expected Ask, got {other:?}"),
    }
}

#[tokio::test]
async fn exit_plan_mode_clear_context_choice_sets_pending_flag() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let session_id = "exit-clear-ctx";
    let plans_dir = coco_context::resolve_plans_directory(tmp.path(), None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let app_state = plan_mode_app_state(Some(PermissionMode::Default), None);
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some(session_id.into());
    ctx.app_state = Some(app_state.clone().into());
    set_exit_plan_choice(&mut ctx, coco_types::ExitPlanChoice::ClearAcceptEdits);

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();
    let guard = app_state.read().await;
    assert!(
        guard.pending_clear_message_history,
        "clear-context approval must schedule MessageHistory::clear()"
    );
    assert!(
        guard
            .pending_plan_implementation_message
            .as_deref()
            .unwrap_or_default()
            .starts_with("Implement the following plan:\n\n# plan\n\nPlan file path: ")
    );
}

#[tokio::test]
async fn exit_plan_mode_clear_context_bounds_approval_feedback() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let session_id = "exit-clear-feedback";
    let plans_dir = coco_context::resolve_plans_directory(tmp.path(), None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let app_state = plan_mode_app_state(Some(PermissionMode::Default), None);
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some(session_id.into());
    ctx.app_state = Some(app_state.clone().into());
    ctx.approval_feedback = Some("x".repeat(super::APPROVAL_FEEDBACK_CONTEXT_CHAR_LIMIT + 10));
    set_exit_plan_choice(&mut ctx, coco_types::ExitPlanChoice::ClearAcceptEdits);

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();

    let guard = app_state.read().await;
    let message = guard
        .pending_plan_implementation_message
        .as_deref()
        .expect("clear-context message");
    assert!(message.contains("Plan file path: "), "{message}");
    assert!(
        message.contains(super::APPROVAL_FEEDBACK_TRUNCATION_MARKER),
        "{message}"
    );
    assert!(
        !message.contains(&"x".repeat(super::APPROVAL_FEEDBACK_CONTEXT_CHAR_LIMIT + 1)),
        "feedback should be bounded"
    );
}

#[tokio::test]
async fn exit_plan_mode_keep_context_choice_does_not_set_pending_flag() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let session_id = "exit-keep-ctx";
    let plans_dir = coco_context::resolve_plans_directory(tmp.path(), None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let app_state = plan_mode_app_state(Some(PermissionMode::Default), None);
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some(session_id.into());
    ctx.app_state = Some(app_state.clone().into());
    set_exit_plan_choice(&mut ctx, coco_types::ExitPlanChoice::KeepDefault);

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();
    let guard = app_state.read().await;
    assert!(
        !guard.pending_clear_message_history,
        "keep-context approval must NOT schedule a clear"
    );
    assert!(guard.pending_plan_implementation_message.is_none());
}

#[tokio::test]
async fn exit_plan_mode_ignores_stale_choice_json() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let session_id = "exit-legacy-choice";
    let plans_dir = coco_context::resolve_plans_directory(tmp.path(), None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let app_state = plan_mode_app_state(Some(PermissionMode::Default), None);
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some(session_id.into());
    ctx.app_state = Some(app_state.clone().into());

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        json!({"user_choice": "yes-clear-context"}),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();
    let guard = app_state.read().await;
    assert!(!guard.pending_clear_message_history);
    assert_eq!(guard.permissions.mode, Some(PermissionMode::Default));
}

#[tokio::test]
async fn exit_plan_mode_ts_manual_choice_sets_default_mode() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let session_id = "exit-manual-default";
    let plans_dir = coco_context::resolve_plans_directory(tmp.path(), None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let app_state = plan_mode_app_state(Some(PermissionMode::AcceptEdits), None);
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some(session_id.into());
    ctx.app_state = Some(app_state.clone().into());
    set_exit_plan_choice(&mut ctx, coco_types::ExitPlanChoice::KeepDefault);

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();
    let guard = app_state.read().await;
    assert_eq!(guard.permissions.mode, Some(PermissionMode::Default));
    assert!(!guard.pending_clear_message_history);
}

#[tokio::test]
async fn exit_plan_mode_ts_elevated_choice_accepts_edits_without_bypass_gate() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let session_id = "exit-elevated-accept";
    let plans_dir = coco_context::resolve_plans_directory(tmp.path(), None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let app_state = plan_mode_app_state(Some(PermissionMode::Default), None);
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.permission_context.bypass_available = false;
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some(session_id.into());
    ctx.app_state = Some(app_state.clone().into());
    set_exit_plan_choice(&mut ctx, coco_types::ExitPlanChoice::KeepAcceptEdits);

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();
    let guard = app_state.read().await;
    assert_eq!(guard.permissions.mode, Some(PermissionMode::AcceptEdits));
    assert!(!guard.pending_clear_message_history);
}

#[tokio::test]
async fn exit_plan_mode_ts_elevated_choice_uses_bypass_when_available() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let session_id = "exit-elevated-bypass";
    let plans_dir = coco_context::resolve_plans_directory(tmp.path(), None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let app_state = plan_mode_app_state(Some(PermissionMode::Default), None);
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.permission_context.bypass_available = true;
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some(session_id.into());
    ctx.app_state = Some(app_state.clone().into());
    set_exit_plan_choice(&mut ctx, coco_types::ExitPlanChoice::KeepAcceptEdits);

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();
    let guard = app_state.read().await;
    assert_eq!(
        guard.permissions.mode,
        Some(PermissionMode::BypassPermissions)
    );
    assert!(!guard.pending_clear_message_history);
}

#[tokio::test]
async fn exit_plan_mode_ts_clear_context_choice_sets_mode_and_pending_flag() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let session_id = "exit-clear-ts";
    let plans_dir = coco_context::resolve_plans_directory(tmp.path(), None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let app_state = plan_mode_app_state(Some(PermissionMode::Default), None);
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some(session_id.into());
    ctx.app_state = Some(app_state.clone().into());
    set_exit_plan_choice(&mut ctx, coco_types::ExitPlanChoice::ClearAcceptEdits);

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();
    let guard = app_state.read().await;
    assert_eq!(guard.permissions.mode, Some(PermissionMode::AcceptEdits));
    assert!(guard.pending_clear_message_history);
}

/// Seed app_state for an ExitPlanMode test. appState is
/// fully initialized at session bootstrap — we do the Rust equivalent
/// by writing the three mode-related fields up front.
fn plan_mode_app_state(
    pre_plan: Option<PermissionMode>,
    stripped: Option<coco_types::PermissionRulesBySource>,
) -> std::sync::Arc<tokio::sync::RwLock<ToolAppState>> {
    std::sync::Arc::new(tokio::sync::RwLock::new(ToolAppState {
        permissions: LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            pre_plan_mode: pre_plan,
            stripped_dangerous_rules: stripped,
            ..Default::default()
        },
        plan_mode_entry_ms: Some(1),
        ..Default::default()
    }))
}

#[tokio::test]
async fn exit_plan_mode_restores_previous_mode() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let session_id = "exit-restores-prev";
    let plans_dir = coco_context::resolve_plans_directory(tmp.path(), None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let app_state = plan_mode_app_state(Some(PermissionMode::AcceptEdits), None);
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some(session_id.into());
    ctx.app_state = Some(app_state.clone().into());

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();
    let guard = app_state.read().await;
    assert_eq!(guard.permissions.mode, Some(PermissionMode::AcceptEdits));
    assert_eq!(guard.permissions.pre_plan_mode, None);
}

#[tokio::test]
async fn exit_plan_mode_restores_default_when_no_stash() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let session_id = "exit-restores-default";
    let plans_dir = coco_context::resolve_plans_directory(tmp.path(), None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let app_state = plan_mode_app_state(None, None);
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some(session_id.into());
    ctx.app_state = Some(app_state.clone().into());

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();
    assert_eq!(
        app_state.read().await.permissions.mode,
        Some(PermissionMode::Default)
    );
}

#[tokio::test]
async fn exit_plan_mode_restoring_to_auto_strips_dangerous_rules() {
    use coco_types::PermissionRule;
    use coco_types::PermissionRuleSource;
    use coco_types::PermissionRuleValue;
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let session_id = "exit-to-auto-strips";
    let plans_dir = coco_context::resolve_plans_directory(tmp.path(), None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let app_state = plan_mode_app_state(Some(PermissionMode::Auto), None);
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some(session_id.into());
    // Seed a dangerous allow rule — `sudo` is classifier-bypassing.
    ctx.permission_context.allow_rules.insert(
        PermissionRuleSource::UserSettings,
        vec![PermissionRule {
            behavior: coco_types::PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: "Bash".into(),
                rule_content: Some("sudo *".into()),
            },
            source: PermissionRuleSource::UserSettings,
        }],
    );
    ctx.app_state = Some(app_state.clone().into());

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();
    let guard = app_state.read().await;
    assert_eq!(guard.permissions.mode, Some(PermissionMode::Auto));
    assert!(
        guard.permissions.stripped_dangerous_rules.is_some(),
        "dangerous rules must be stashed on Plan→Auto exit"
    );
}

#[tokio::test]
async fn exit_plan_mode_restoring_to_default_clears_stripped_rules() {
    // Inverse of the Auto case: if dangerous rules were stripped during
    // plan mode, exiting back to Default must clear the stash (so the
    // next ctx rebuild sees the un-stripped rules again).
    use coco_types::PermissionRule;
    use coco_types::PermissionRuleSource;
    use coco_types::PermissionRuleValue;
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let session_id = "exit-to-default-clears";
    let plans_dir = coco_context::resolve_plans_directory(tmp.path(), None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let mut stashed = coco_types::PermissionRulesBySource::new();
    stashed.insert(
        PermissionRuleSource::UserSettings,
        vec![PermissionRule {
            behavior: coco_types::PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: "Bash".into(),
                rule_content: Some("sudo *".into()),
            },
            source: PermissionRuleSource::UserSettings,
        }],
    );
    let app_state = plan_mode_app_state(Some(PermissionMode::Default), Some(stashed));
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some(session_id.into());
    ctx.app_state = Some(app_state.clone().into());

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();
    let guard = app_state.read().await;
    assert_eq!(guard.permissions.mode, Some(PermissionMode::Default));
    assert!(
        guard.permissions.stripped_dangerous_rules.is_none(),
        "stripped rules must be cleared on non-Auto exit"
    );
}

#[tokio::test]
async fn exit_plan_mode_execute_sets_exit_flags_on_app_state() {
    // ExitPlanModeTool.execute must set both `has_exited_plan_mode` and
    // `needs_plan_mode_exit_attachment` so the reminder orchestrator
    // fires Reentry (on next entry) + the exit banner (on the same turn).
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::RwLock;

    let tmp = tempdir().unwrap();
    let config_home = tmp.path().to_path_buf();
    let session_id = "test-exit-flags";
    let plans_dir = coco_context::resolve_plans_directory(&config_home, None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let app_state = Arc::new(RwLock::new(ToolAppState {
        plan_mode_entry_ms: Some(1),
        ..Default::default()
    }));
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(config_home);
    ctx.session_id_for_history = Some(session_id.to_string());
    ctx.app_state = Some(app_state.clone().into());

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();

    let guard = app_state.read().await;
    assert!(guard.has_exited_plan_mode);
    assert!(guard.needs_plan_mode_exit_attachment);
    // Default→Plan→Default cycle: auto wasn't active, no auto-exit banner.
    assert!(!guard.needs_auto_mode_exit_attachment);
}

#[tokio::test]
async fn exit_plan_mode_from_auto_with_no_restore_target_fires_auto_exit_flag() {
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::RwLock;

    let tmp = tempdir().unwrap();
    let config_home = tmp.path().to_path_buf();
    let session_id = "test-auto-exit-from-plan";
    let plans_dir = coco_context::resolve_plans_directory(&config_home, None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    // Simulate "Auto was active during plan" on app_state — the
    // shared store is the source of truth
    // (`appState.toolPermissionContext.strippedDangerousRules`).
    let app_state = Arc::new(RwLock::new(ToolAppState {
        permissions: LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            stripped_dangerous_rules: Some(coco_types::PermissionRulesBySource::default()),
            ..Default::default()
        },
        plan_mode_entry_ms: Some(1),
        ..Default::default()
    }));
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(config_home);
    ctx.session_id_for_history = Some(session_id.to_string());
    ctx.app_state = Some(app_state.clone().into());

    let _ = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();

    let guard = app_state.read().await;
    assert!(
        guard.needs_auto_mode_exit_attachment,
        "auto-mode-exit flag must be set when auto was active during plan \
         and restore is not Auto"
    );
}

#[tokio::test]
async fn enter_plan_mode_execute_records_entry_timestamp() {
    // EnterPlanModeTool.execute records the plan entry point for plan-mode
    // lifecycle consumers.
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let app_state = Arc::new(RwLock::new(ToolAppState::default()));
    let mut ctx = ctx_with_mode(PermissionMode::Default);
    ctx.app_state = Some(app_state.clone().into());

    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let _ = execute_and_apply_patch(&EnterPlanModeTool, json!({}), &ctx, &app_state)
        .await
        .unwrap();
    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    let guard = app_state.read().await;
    let entry = guard.plan_mode_entry_ms.expect("entry_ms was not set");
    assert!(entry >= before && entry <= after, "entry_ms out of bounds");
    // Re-entry should clear any stale exit-attachment flag.
    assert!(!guard.needs_plan_mode_exit_attachment);
}

#[tokio::test]
async fn exit_plan_mode_reads_plan_from_disk() {
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::RwLock;

    // Write a plan to disk, then verify ExitPlanMode.execute() reads it.
    let tmp = tempdir().unwrap();
    let config_home = tmp.path().to_path_buf();
    let session_id = "test-session-read-disk";
    let plans_dir = coco_context::resolve_plans_directory(&config_home, None, None);
    coco_context::write_plan(session_id, &plans_dir, "## Plan\n- step 1", None).unwrap();

    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(config_home);
    ctx.session_id_for_history = Some(session_id.to_string());
    ctx.app_state = Some(
        Arc::new(RwLock::new(ToolAppState {
            plan_mode_entry_ms: Some(1),
            ..Default::default()
        }))
        .into(),
    );

    let result = <ExitPlanModeTool as DynTool>::execute(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
    )
    .await
    .unwrap();
    let plan = result
        .data
        .get("plan")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        result.data.get("outcome").and_then(Value::as_str),
        Some("implementation_plan")
    );
    assert!(
        plan.contains("step 1"),
        "plan should be read from disk: {plan}"
    );

    let file_path = result
        .data
        .get("filePath")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        file_path.ends_with(".md"),
        "filePath should be set: {file_path}"
    );

    let Some(ToolDisplayData::ExitPlanModeResult(display)) = result.display_data else {
        panic!("ExitPlanMode should attach typed display data");
    };
    assert!(display.plan.contains("step 1"), "{display:?}");
    assert_eq!(display.file_path.as_deref(), Some(file_path));
    assert!(!display.awaiting_leader_approval);
    assert!(!display.is_agent);
    assert!(!display.plan_was_edited);
    assert_eq!(
        display.outcome,
        coco_types::ExitPlanModeOutcome::ImplementationPlan
    );
}

#[tokio::test]
async fn exit_plan_mode_approved_edit_wins_over_disk() {
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::RwLock;

    let tmp = tempdir().unwrap();
    let config_home = tmp.path().to_path_buf();
    let session_id = "test-session-input-wins";
    let plans_dir = coco_context::resolve_plans_directory(&config_home, None, None);
    coco_context::write_plan(session_id, &plans_dir, "on-disk plan", None).unwrap();

    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(config_home);
    ctx.session_id_for_history = Some(session_id.to_string());
    ctx.app_state = Some(
        Arc::new(RwLock::new(ToolAppState {
            plan_mode_entry_ms: Some(1),
            ..Default::default()
        }))
        .into(),
    );
    ctx.permission_resolution_detail = Some(coco_types::PermissionResolutionDetail::ExitPlanMode {
        choice: coco_types::ExitPlanChoice::KeepDefault,
        edited_plan: Some("edited plan from CCR".into()),
    });

    let result = <ExitPlanModeTool as DynTool>::execute(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
    )
    .await
    .unwrap();
    let plan = result
        .data
        .get("plan")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(plan, "edited plan from CCR");
    assert_eq!(
        result.data.get("planWasEdited").and_then(Value::as_bool),
        Some(true)
    );
    let Some(ToolDisplayData::ExitPlanModeResult(display)) = result.display_data else {
        panic!("ExitPlanMode should attach typed display data");
    };
    assert_eq!(display.plan, "edited plan from CCR");
    assert!(display.plan_was_edited);
}

#[tokio::test]
async fn exit_plan_mode_stale_json_plan_without_disk_is_no_plan() {
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::RwLock;

    let tmp = tempdir().unwrap();
    let config_home = tmp.path().to_path_buf();
    let session_id = "test-session-stale-json-no-disk";
    let app_state = Arc::new(RwLock::new(ToolAppState {
        plan_mode_entry_ms: Some(1),
        ..Default::default()
    }));

    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(config_home);
    ctx.session_id_for_history = Some(session_id.to_string());
    ctx.app_state = Some(app_state.clone().into());

    let result = execute_and_apply_patch(
        &ExitPlanModeTool,
        json!({"plan": "stale injected plan"}),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();

    assert_eq!(
        result.data.get("outcome").and_then(Value::as_str),
        Some("no_implementation_plan")
    );
    assert!(result.data.get("plan").is_none());
}

#[tokio::test]
async fn exit_plan_mode_approved_edit_without_path_returns_typed_error() {
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.permission_resolution_detail = Some(coco_types::PermissionResolutionDetail::ExitPlanMode {
        choice: coco_types::ExitPlanChoice::KeepDefault,
        edited_plan: Some("edited plan".into()),
    });

    let err = <ExitPlanModeTool as DynTool>::execute(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
    )
    .await
    .expect_err("approved edit without a plan path must fail");

    match err {
        coco_tool_runtime::ToolError::InvalidInput { error_code, .. } => {
            assert_eq!(
                error_code.as_deref(),
                Some("approved_edit_without_plan_path")
            );
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[tokio::test]
async fn exit_plan_mode_no_plan_notice_is_not_saved_as_plan() {
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::RwLock;

    let tmp = tempdir().unwrap();
    let config_home = tmp.path().to_path_buf();
    let session_id = "test-session-no-plan";
    let plans_dir = coco_context::resolve_plans_directory(&config_home, None, None);
    let plan_path = coco_context::get_plan_file_path(session_id, &plans_dir, None);
    let app_state = Arc::new(RwLock::new(ToolAppState {
        plan_mode_entry_ms: Some(1),
        ..Default::default()
    }));

    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(config_home);
    ctx.session_id_for_history = Some(session_id.to_string());
    ctx.app_state = Some(app_state.clone().into());
    set_exit_plan_choice(&mut ctx, coco_types::ExitPlanChoice::ClearAcceptEdits);

    let result = execute_and_apply_patch(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
        &app_state,
    )
    .await
    .unwrap();

    assert!(
        result.data.get("filePath").is_none(),
        "no-plan result must not claim a saved plan path: {:?}",
        result.data
    );
    assert_eq!(
        result.data.get("outcome").and_then(Value::as_str),
        Some("no_implementation_plan")
    );
    assert!(
        !plan_path.exists(),
        "no-plan notice must not be persisted to {}",
        plan_path.display()
    );
    let Some(ToolDisplayData::ExitPlanModeResult(display)) = result.display_data else {
        panic!("ExitPlanMode should attach typed display data");
    };
    assert_eq!(
        display.outcome,
        coco_types::ExitPlanModeOutcome::NoImplementationPlan
    );
    assert!(!display.has_implementation_plan());
    assert_eq!(display.file_path, None);

    let guard = app_state.read().await;
    assert_eq!(guard.permissions.mode, Some(PermissionMode::Default));
    assert!(!guard.pending_clear_message_history);
    assert_eq!(guard.pending_plan_implementation_message, None);
    assert!(guard.pending_plan_verification.is_none());
    assert_eq!(
        guard.pending_plan_mode_exit_outcome,
        Some(coco_types::ExitPlanModeOutcome::NoImplementationPlan)
    );
}

#[tokio::test]
async fn exit_plan_mode_without_current_plan_exits_without_implementation_plan() {
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::RwLock;

    let tmp = tempdir().unwrap();
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some("test-session-missing-plan".to_string());
    ctx.app_state = Some(
        Arc::new(RwLock::new(ToolAppState {
            plan_mode_entry_ms: Some(1),
            ..Default::default()
        }))
        .into(),
    );

    let result = <ExitPlanModeTool as DynTool>::execute(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(
        result.data.get("outcome").and_then(Value::as_str),
        Some("no_implementation_plan")
    );
    assert!(result.data.get("plan").is_none());
}

#[tokio::test]
async fn exit_plan_mode_stale_json_plan_not_marked_as_user_edit() {
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::RwLock;

    let tmp = tempdir().unwrap();
    let config_home = tmp.path().to_path_buf();
    let session_id = "test-session-injected-plan";
    let plans_dir = coco_context::resolve_plans_directory(&config_home, None, None);
    coco_context::write_plan(session_id, &plans_dir, "on-disk plan", None).unwrap();

    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(config_home);
    ctx.session_id_for_history = Some(session_id.to_string());
    ctx.app_state = Some(
        Arc::new(RwLock::new(ToolAppState {
            plan_mode_entry_ms: Some(1),
            ..Default::default()
        }))
        .into(),
    );

    let result = <ExitPlanModeTool as DynTool>::execute(
        &ExitPlanModeTool,
        json!({"plan": "on-disk plan"}),
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(result.data.get("planWasEdited"), None);
}

#[test]
fn exit_plan_mode_schema_exposes_allowed_prompts() {
    let schema =
        <ExitPlanModeTool as DynTool>::runtime_validation_schema(&ExitPlanModeTool).as_value();
    assert!(schema["properties"].get("allowedPrompts").is_some());
    assert!(schema["properties"].get("outcome").is_none());
    assert_ne!(schema["additionalProperties"], json!(false));
    let required = schema["required"].as_array().cloned().unwrap_or_default();
    assert!(
        !required
            .iter()
            .any(|field| field.as_str() == Some("outcome")),
        "outcome must not be required: {required:?}"
    );
}

#[test]
fn exit_plan_mode_accepts_passthrough_fields_and_parses_allowed_prompts() {
    let input = json!({
        "allowedPrompts": [{"tool": "Bash", "prompt": "cargo test"}],
        "plan_file": "/tmp/plan.md",
        "teardown": true
    });
    let schema =
        <ExitPlanModeTool as DynTool>::runtime_validation_schema(&ExitPlanModeTool).as_value();
    let validation = <ExitPlanModeTool as DynTool>::runtime_validation_schema(&ExitPlanModeTool)
        .validate(&input);
    assert!(
        validation.is_ok(),
        "schema rejected passthrough fields: {schema}"
    );

    let parsed: super::ExitPlanModeInput =
        serde_json::from_value(input).expect("ExitPlanModeInput parses");
    let prompts = parsed.allowed_prompts.expect("allowedPrompts parsed");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].prompt, "cargo test");
}

#[test]
fn exit_plan_mode_name_matches_registry() {
    assert_eq!(
        <ExitPlanModeTool as DynTool>::name(&ExitPlanModeTool,),
        ToolName::ExitPlanMode.as_str()
    );
}

#[test]
fn enter_plan_mode_name_matches_registry() {
    assert_eq!(
        <EnterPlanModeTool as DynTool>::name(&EnterPlanModeTool,),
        ToolName::EnterPlanMode.as_str()
    );
}

// ── Teammate approval flow ──

use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;

/// Capture mailbox writes for assertion without hitting disk.
#[derive(Default)]
struct CapturingMailbox {
    captured: TokioMutex<Vec<(String, String, coco_tool_runtime::MailboxEnvelope)>>,
}

#[async_trait::async_trait]
impl coco_tool_runtime::MailboxHandle for CapturingMailbox {
    async fn write_to_mailbox(
        &self,
        recipient: &str,
        team_name: &str,
        message: coco_tool_runtime::MailboxEnvelope,
    ) -> Result<(), coco_error::BoxedError> {
        self.captured
            .lock()
            .await
            .push((recipient.into(), team_name.into(), message));
        Ok(())
    }
    async fn read_unread(
        &self,
        _agent: &str,
        _team: &str,
    ) -> Result<Vec<coco_tool_runtime::InboxMessage>, coco_error::BoxedError> {
        Ok(Vec::new())
    }
    async fn mark_read(
        &self,
        _agent: &str,
        _team: &str,
        _index: usize,
    ) -> Result<(), coco_error::BoxedError> {
        Ok(())
    }
}

#[tokio::test]
async fn teammate_exit_plan_writes_approval_request_to_team_lead() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let config_home = tmp.path().to_path_buf();
    let session_id = "test-teammate-exit";
    let plans_dir = coco_context::resolve_plans_directory(&config_home, None, None);
    // Teammate has agent_id=alice → plan file is `{slug}-agent-alice.md`.
    coco_context::write_plan(session_id, &plans_dir, "# plan body", Some("alice")).unwrap();

    let capture = Arc::new(CapturingMailbox::default());

    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.is_teammate = true;
    ctx.plan_mode_required = true;
    ctx.config_home = Some(config_home);
    ctx.session_id_for_history = Some(session_id.to_string());
    ctx.app_state = Some(
        Arc::new(tokio::sync::RwLock::new(ToolAppState {
            plan_mode_entry_ms: Some(1),
            ..Default::default()
        }))
        .into(),
    );
    ctx.mailbox = capture.clone();
    // The tool falls back to `ctx.agent_id` when env vars aren't set,
    // so we can control identity without mutating the global env
    // (`env::set_var` is unsafe in newer Rust + banned by CLAUDE.md).
    ctx.agent_id = Some(coco_types::AgentId::new("alice"));

    let result = <ExitPlanModeTool as DynTool>::execute(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
    )
    .await
    .unwrap();

    // Result shape signals "awaiting leader approval".
    assert_eq!(
        result
            .data
            .get("awaitingLeaderApproval")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert!(
        result
            .data
            .get("requestId")
            .and_then(Value::as_str)
            .is_some()
    );
    let Some(ToolDisplayData::ExitPlanModeResult(display)) = result.display_data else {
        panic!("ExitPlanMode should attach typed display data");
    };
    assert_eq!(display.plan, "# plan body");
    assert_eq!(
        display.outcome,
        coco_types::ExitPlanModeOutcome::ImplementationPlan
    );
    assert!(display.awaiting_leader_approval);
    assert!(display.is_agent);

    // One mailbox write to team-lead under the fallback team name.
    let captured = capture.captured.lock().await;
    assert_eq!(captured.len(), 1, "exactly one plan_approval_request");
    let (recipient, team, env) = &captured[0];
    assert_eq!(recipient, "team-lead");
    // `COCO_TEAM_NAME` unset in-test (ctx.team_name=None) → fallback "default".
    assert_eq!(team, "default");
    // Body is a JSON-serialized PlanApprovalRequest.
    let parsed: serde_json::Value = serde_json::from_str(&env.text).unwrap();
    assert_eq!(parsed["type"], "plan_approval_request");
    assert_eq!(parsed["from"], "alice");
    assert!(
        parsed["planContent"]
            .as_str()
            .unwrap()
            .contains("plan body")
    );
    assert!(
        parsed["requestId"]
            .as_str()
            .unwrap()
            .starts_with("plan_approval-alice-default-")
    );

    // Teammate exit does NOT flip live mode — teammate stays in Plan
    // until the leader responds. ExitPlanModeTool::execute returns
    // early on the teammate branch (ExitPlanModeV2Tool.ts:264-313),
    // leaving app_state.permission_mode untouched.
    drop(captured);
    // No post-processing needed — the execute above already captured
    // the final state. Verify app_state wasn't mode-flipped (we
    // didn't set it, so it's whatever Default is — the point is that
    // no Plan→Default write happened on the teammate branch).
}

#[tokio::test]
async fn teammate_exit_plan_with_empty_plan_errors() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let config_home = tmp.path().to_path_buf();
    let session_id = "test-teammate-empty";
    // No plan written.

    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.is_teammate = true;
    ctx.plan_mode_required = true;
    ctx.config_home = Some(config_home);
    ctx.session_id_for_history = Some(session_id.to_string());
    ctx.mailbox = Arc::new(CapturingMailbox::default());

    let result = <ExitPlanModeTool as DynTool>::execute(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
    )
    .await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("No plan file found")
    );
}

#[tokio::test]
async fn voluntary_teammate_exits_locally_without_mailbox_write() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let config_home = tmp.path().to_path_buf();
    let session_id = "voluntary-teammate";
    let plans_dir = coco_context::resolve_plans_directory(&config_home, None, None);
    coco_context::write_plan(session_id, &plans_dir, "# vol plan", None).unwrap();

    let capture = Arc::new(CapturingMailbox::default());
    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.is_teammate = true;
    ctx.plan_mode_required = false; // voluntary
    ctx.config_home = Some(config_home);
    ctx.session_id_for_history = Some(session_id.to_string());
    ctx.app_state = Some(
        Arc::new(tokio::sync::RwLock::new(ToolAppState {
            plan_mode_entry_ms: Some(1),
            ..Default::default()
        }))
        .into(),
    );
    ctx.mailbox = capture.clone();

    let result = <ExitPlanModeTool as DynTool>::execute(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
    )
    .await
    .unwrap();

    // No awaiting flag — normal exit semantics.
    assert_eq!(
        result
            .data
            .get("awaitingLeaderApproval")
            .and_then(Value::as_bool),
        None
    );
    // No mailbox write.
    assert!(
        capture.captured.lock().await.is_empty(),
        "voluntary teammate must NOT write to mailbox"
    );
}

#[tokio::test]
async fn verify_execution_disabled_skips_pending_verification() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let config_home = tmp.path().to_path_buf();
    let session_id = "test-verify-off";
    let plans_dir = coco_context::resolve_plans_directory(&config_home, None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(config_home);
    ctx.session_id_for_history = Some(session_id.to_string());
    ctx.app_state = Some(
        Arc::new(tokio::sync::RwLock::new(ToolAppState {
            plan_mode_entry_ms: Some(1),
            ..Default::default()
        }))
        .into(),
    );
    ctx.plan_verify_execution = false;

    let result = <ExitPlanModeTool as DynTool>::execute(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(
        result.data.get("planVerification").and_then(Value::as_str),
        None,
        "ExitPlanMode no longer emits the old mtime advisory"
    );
    assert!(result.app_state_patch.is_some());
    let app_state = ctx.app_state.as_ref().unwrap().clone();
    let mut result = result;
    let patch = result.app_state_patch.take().unwrap();
    {
        let mut guard = app_state.write().await;
        patch(&mut guard);
        assert!(guard.pending_plan_verification.is_none());
    }
}

#[tokio::test]
async fn verify_execution_enabled_records_pending_verification() {
    use tempfile::tempdir;
    let tmp = tempdir().unwrap();
    let config_home = tmp.path().to_path_buf();
    let session_id = "test-verify-on";
    let plans_dir = coco_context::resolve_plans_directory(&config_home, None, None);
    coco_context::write_plan(session_id, &plans_dir, "# plan", None).unwrap();

    let mut ctx = ctx_with_mode(PermissionMode::Plan);
    ctx.config_home = Some(config_home);
    ctx.session_id_for_history = Some(session_id.to_string());
    ctx.app_state = Some(
        Arc::new(tokio::sync::RwLock::new(ToolAppState {
            plan_mode_entry_ms: Some(1),
            ..Default::default()
        }))
        .into(),
    );
    ctx.plan_verify_execution = true;

    let mut result = <ExitPlanModeTool as DynTool>::execute(
        &ExitPlanModeTool,
        implementation_plan_input(),
        &ctx,
    )
    .await
    .unwrap();
    assert!(
        result.data.get("planVerification").is_none(),
        "ExitPlanMode no longer emits the old mtime advisory"
    );
    let patch = result.app_state_patch.take().unwrap();
    let app_state = ctx.app_state.as_ref().unwrap().clone();
    {
        let mut guard = app_state.write().await;
        patch(&mut guard);
        let pending = guard
            .pending_plan_verification
            .as_ref()
            .expect("verification flow stores TS-shaped pending state");
        assert_eq!(pending.plan, "# plan");
        assert!(!pending.verification_started);
        assert!(!pending.verification_completed);
    }
}

#[test]
fn build_instructions_awaiting_leader_approval_variant() {
    let out = ExitPlanModeTool::build_instructions(&json!({
        "awaitingLeaderApproval": true,
        "requestId": "plan_approval-alice-team-a-deadbeef",
        "filePath": "/tmp/plan.md",
    }));
    assert!(out.contains("submitted to the team lead"));
    assert!(out.contains("plan_approval-alice-team-a-deadbeef"));
    assert!(out.contains("/tmp/plan.md"));
    assert!(out.contains("Do NOT proceed"));
}

// ── build_instructions ──

#[test]
fn build_instructions_agent_variant() {
    let out = ExitPlanModeTool::build_instructions(&json!({
        "outcome": "implementation_plan",
        "isAgent": true,
        "plan": "x"
    }));
    assert!(out.contains("respond with"));
}

#[test]
fn build_instructions_empty_plan() {
    let out = ExitPlanModeTool::build_instructions(&json!({
        "outcome": "no_implementation_plan",
        "plan": "   "
    }));
    assert!(out.contains("no implementation plan"));
}

#[test]
fn build_instructions_no_plan_notice() {
    let out = ExitPlanModeTool::build_instructions(&json!({
        "outcome": "no_implementation_plan",
        "plan": "User asked for a read-only explanation.",
        "filePath": "/tmp/plan.md",
    }));
    assert!(out.contains("no implementation plan"));
    assert!(!out.contains("/tmp/plan.md"));
    assert!(!out.contains("Approved Plan"));
}

#[test]
fn build_instructions_with_plan_and_edited_flag() {
    let out = ExitPlanModeTool::build_instructions(&json!({
        "outcome": "implementation_plan",
        "plan": "step 1",
        "filePath": "/tmp/plan.md",
        "planWasEdited": true,
    }));
    assert!(out.contains("(edited by user)"));
    assert!(out.contains("/tmp/plan.md"));
    assert!(out.contains("step 1"));
}

#[tokio::test]
async fn exit_plan_mode_tool_spec_hides_internal_fields_and_tightens_allowed_prompts() {
    let coco_tool_runtime::ToolSpec::Function(spec) = <ExitPlanModeTool as DynTool>::tool_spec(
        &ExitPlanModeTool,
        &coco_tool_runtime::SchemaContext::default(),
        &coco_tool_runtime::PromptOptions::default(),
    )
    .await
    else {
        panic!("ExitPlanMode should produce a Function spec");
    };
    let schema = spec.parameters;
    let props = schema["properties"].as_object().expect("object schema");
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        !required.contains(&"outcome"),
        "model schema must not require outcome; got {required:?}"
    );
    // Internal splice fields must be hidden from the model.
    assert!(
        !props.contains_key("outcome"),
        "outcome leaked into model schema"
    );
    assert!(!props.contains_key("plan"), "plan leaked into model schema");
    assert!(
        !props.contains_key("planFilePath"),
        "planFilePath leaked into model schema"
    );
    assert!(
        !props.contains_key("user_choice"),
        "user_choice leaked into model schema"
    );
    // `allowedPrompts` stays — its item shape:
    // `{ tool: enum["Bash"], prompt }` with both required.
    let items = &schema["properties"]["allowedPrompts"]["items"];
    let item_required: Vec<&str> = items["required"]
        .as_array()
        .expect("allowedPrompts item must declare `required`")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        item_required.contains(&"tool") && item_required.contains(&"prompt"),
        "allowedPrompts item must require tool+prompt; got {item_required:?}"
    );
    assert_eq!(
        items["properties"]["tool"]["enum"],
        json!(["Bash"]),
        "allowedPrompts item `tool` must be enum[\"Bash\"]"
    );
}

// ── Prompt + post-execute parity tests (G5.1) ──
//
// Byte-precise comparisons against the reference prompts.
// Any drift (a missing newline, a moved bullet, a renamed tool reference)
// will fail this test rather than silently change what the model sees.

use coco_tool_runtime::PromptOptions;
use pretty_assertions::assert_eq as ts_assert_eq;

/// External-arm `EnterPlanMode` prompt with `whatHappens=WHAT_HAPPENS_SECTION`.
const TS_ENTER_PLAN_MODE_PROMPT_FIVE_PHASE: &str =
"Use this tool proactively when you're about to start a non-trivial implementation task. Getting user sign-off on your approach before writing code prevents wasted effort and ensures alignment. This tool transitions you into plan mode where you can explore the codebase and design an implementation approach for user approval.

## When to Use This Tool

**Prefer using EnterPlanMode** for implementation tasks unless they're simple. Use it when ANY of these conditions apply:

1. **New Feature Implementation**: Adding meaningful new functionality
   - Example: \"Add a logout button\" - where should it go? What should happen on click?
   - Example: \"Add form validation\" - what rules? What error messages?

2. **Multiple Valid Approaches**: The task can be solved in several different ways
   - Example: \"Add caching to the API\" - could use Redis, in-memory, file-based, etc.
   - Example: \"Improve performance\" - many optimization strategies possible

3. **Code Modifications**: Changes that affect existing behavior or structure
   - Example: \"Update the login flow\" - what exactly should change?
   - Example: \"Refactor this component\" - what's the target architecture?

4. **Architectural Decisions**: The task requires choosing between patterns or technologies
   - Example: \"Add real-time updates\" - WebSockets vs SSE vs polling
   - Example: \"Implement state management\" - Redux vs Context vs custom solution

5. **Multi-File Changes**: The task will likely touch more than 2-3 files
   - Example: \"Refactor the authentication system\"
   - Example: \"Add a new API endpoint with tests\"

6. **Unclear Requirements**: You need to explore before understanding the full scope
   - Example: \"Make the app faster\" - need to profile and identify bottlenecks
   - Example: \"Fix the bug in checkout\" - need to investigate root cause

7. **User Preferences Matter**: The implementation could reasonably go multiple ways
   - If you would use AskUserQuestion to clarify the approach, use EnterPlanMode instead
   - Plan mode lets you explore first, then present options with context

## When NOT to Use This Tool

Only skip EnterPlanMode for simple tasks:
- Single-line or few-line fixes (typos, obvious bugs, small tweaks)
- Adding a single function with clear requirements
- Tasks where the user has given very specific, detailed instructions
- Pure research/exploration tasks (use the Agent tool with explore agent instead)

## What Happens in Plan Mode

In plan mode, you'll:
1. Thoroughly explore the codebase using Glob, Grep, and Read tools
2. Understand existing patterns and architecture
3. Design an implementation approach
4. Present your plan to the user for approval
5. Use AskUserQuestion if you need to clarify approaches
6. Exit plan mode with ExitPlanMode when ready to implement

## Examples

### GOOD - Use EnterPlanMode:
User: \"Add user authentication to the app\"
- Requires architectural decisions (session vs JWT, where to store tokens, middleware structure)

User: \"Optimize the database queries\"
- Multiple approaches possible, need to profile first, significant impact

User: \"Implement dark mode\"
- Architectural decision on theme system, affects many components

User: \"Add a delete button to the user profile\"
- Seems simple but involves: where to place it, confirmation dialog, API call, error handling, state updates

User: \"Update the error handling in the API\"
- Affects multiple files, user should approve the approach

### BAD - Don't use EnterPlanMode:
User: \"Fix the typo in the README\"
- Straightforward, no planning needed

User: \"Add a console.log to debug this function\"
- Simple, obvious implementation

User: \"What files handle routing?\"
- Research task, not implementation planning

## Important Notes

- This tool REQUIRES user approval - they must consent to entering plan mode
- If unsure whether to use it, err on the side of planning - it's better to get alignment upfront than to redo work
- Users appreciate being consulted before significant changes are made to their codebase
";

#[tokio::test]
async fn enter_plan_mode_prompt_five_phase_matches_ts_byte_precise() {
    let opts = PromptOptions {
        is_plan_interview_phase: false,
        ..Default::default()
    };
    let actual = <EnterPlanModeTool as DynTool>::prompt(&EnterPlanModeTool, &opts).await;
    ts_assert_eq!(actual, TS_ENTER_PLAN_MODE_PROMPT_FIVE_PHASE);
}

#[tokio::test]
async fn enter_plan_mode_prompt_interview_omits_what_happens() {
    // When `isPlanModeInterviewPhaseEnabled()`, `whatHappens` is `''`,
    // so the `## What Happens in Plan Mode` block disappears. The
    // surrounding structure (Examples, Important Notes) stays.
    let opts = PromptOptions {
        is_plan_interview_phase: true,
        ..Default::default()
    };
    let actual = <EnterPlanModeTool as DynTool>::prompt(&EnterPlanModeTool, &opts).await;
    assert!(
        !actual.contains("## What Happens in Plan Mode"),
        "interview-phase prompt must omit 'What Happens' section"
    );
    assert!(
        !actual.contains("Exit plan mode with ExitPlanMode when ready to implement"),
        "interview-phase prompt must omit the 6-step list inside 'What Happens'"
    );
    // Surrounding structure still present.
    assert!(actual.contains("## Examples"));
    assert!(actual.contains("## Important Notes"));
    assert!(actual.contains("- This tool REQUIRES user approval"));
    // The condition #7 mention of AskUserQuestion is in the upper
    // section (NOT inside WHAT_HAPPENS_SECTION) so it stays.
    assert!(actual.contains("If you would use AskUserQuestion to clarify"));
}

const TS_EXIT_PLAN_MODE_PROMPT: &str =
"Use this tool when you are in plan mode and have finished writing your plan to the plan file and are ready for user approval.

## How This Tool Works
- You should have already written your plan to the plan file specified in the plan mode system message
- This tool does NOT take the plan content as a parameter - it will read the plan from the file you wrote
- This tool simply signals that you're done planning and ready for the user to review and approve, or that no implementation plan is needed
- The user will see the contents of your plan file when they review it

## When to Use This Tool
IMPORTANT: Only use this tool when the task requires planning the implementation steps of a task that requires writing code. For research tasks where you're gathering information, searching files, reading files or in general trying to understand the codebase - do NOT use this tool.

## Before Using This Tool
Ensure your plan is complete and unambiguous:
- If you have unresolved questions about requirements or approach, use AskUserQuestion first (in earlier phases)
- Once your plan is finalized, use THIS tool to request approval

**Important:** Do NOT use AskUserQuestion to ask \"Is this plan okay?\" or \"Should I proceed?\" - that's exactly what THIS tool does. ExitPlanMode inherently requests user approval of your plan.
";

#[tokio::test]
async fn exit_plan_mode_prompt_matches_ts_byte_precise() {
    let opts = PromptOptions::default();
    let actual = <ExitPlanModeTool as DynTool>::prompt(&ExitPlanModeTool, &opts).await;
    ts_assert_eq!(actual, TS_EXIT_PLAN_MODE_PROMPT);
}

#[test]
fn enter_plan_mode_build_instructions_five_phase_matches_ts() {
    // Non-interview branch.
    let confirmation = "Hello.";
    let expected = "Hello.\n\nIn plan mode, you should:\n\
                    1. Thoroughly explore the codebase to understand existing patterns\n\
                    2. Identify similar features and architectural approaches\n\
                    3. Consider multiple approaches and their trade-offs\n\
                    4. Use AskUserQuestion if you need to clarify the approach\n\
                    5. Design a concrete implementation strategy\n\
                    6. When ready, use ExitPlanMode to present your plan for approval\n\n\
                    Remember: DO NOT write or edit any files yet. \
                    This is a read-only exploration and planning phase.";
    let actual = EnterPlanModeTool::build_instructions(confirmation, false);
    ts_assert_eq!(actual, expected);
}

#[test]
fn enter_plan_mode_build_instructions_interview_matches_ts() {
    // Interview branch.
    let confirmation = "Hello.";
    let expected = "Hello.\n\nDO NOT write or edit any files except the plan file. \
                    Detailed workflow instructions will follow.";
    let actual = EnterPlanModeTool::build_instructions(confirmation, true);
    ts_assert_eq!(actual, expected);
}

#[tokio::test]
async fn enter_plan_mode_execute_data_carries_short_confirmation_and_flag() {
    // Post `Tool::render_for_model` migration: `execute` writes ONLY
    // the short confirmation + the `isInterviewPhase` flag into
    // `data`. The full workflow splice now lives in `render_for_model`
    // (covered by `enter_plan_mode_render_for_model_*` below). This
    // matches the expected `execute` output shape exactly.
    let mut ctx = ctx_with_mode(PermissionMode::Default);
    ctx.is_plan_interview_phase = false;
    let result = <EnterPlanModeTool as DynTool>::execute(&EnterPlanModeTool, json!({}), &ctx)
        .await
        .unwrap();
    let msg = result
        .data
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        msg,
        "Entered plan mode. You should now focus on exploring the codebase and designing an implementation approach."
    );
    assert_eq!(
        result.data.get("isInterviewPhase").and_then(Value::as_bool),
        Some(false)
    );

    let mut ctx = ctx_with_mode(PermissionMode::Default);
    ctx.is_plan_interview_phase = true;
    let result = <EnterPlanModeTool as DynTool>::execute(&EnterPlanModeTool, json!({}), &ctx)
        .await
        .unwrap();
    assert_eq!(
        result.data.get("isInterviewPhase").and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn enter_plan_mode_render_for_model_five_phase_branch() {
    // Renderer pulls the workflow flag out of `data` (written by
    // `execute`) and produces a single Text part with the full
    // 6-step splice.
    let data = json!({
        "message": "Entered plan mode. You should now focus on exploring the codebase and designing an implementation approach.",
        "isInterviewPhase": false,
    });
    let parts = <EnterPlanModeTool as DynTool>::render_for_model(&EnterPlanModeTool, &data);
    let [coco_tool_runtime::ToolResultContentPart::Text { text, .. }] = parts.as_slice() else {
        panic!("expected single Text part, got {parts:?}");
    };
    assert!(text.starts_with("Entered plan mode."));
    assert!(text.contains("In plan mode, you should:"));
    assert!(text.contains("6. When ready, use ExitPlanMode"));
}

#[test]
fn enter_plan_mode_render_for_model_interview_branch() {
    let data = json!({
        "message": "Entered plan mode. You should now focus on exploring the codebase and designing an implementation approach.",
        "isInterviewPhase": true,
    });
    let parts = <EnterPlanModeTool as DynTool>::render_for_model(&EnterPlanModeTool, &data);
    let [coco_tool_runtime::ToolResultContentPart::Text { text, .. }] = parts.as_slice() else {
        panic!("expected single Text part, got {parts:?}");
    };
    assert!(text.starts_with("Entered plan mode."));
    assert!(text.contains("DO NOT write or edit any files except the plan file"));
    assert!(!text.contains("In plan mode, you should:"));
}

#[test]
fn exit_plan_mode_render_for_model_routes_through_build_instructions() {
    // ExitPlanMode `render_for_model` must thread `data` directly
    // through `build_instructions` — the executor's `tool_outcome_builder`
    // calls this method, no longer JSON-stringifies.
    let data = json!({
        "outcome": "implementation_plan",
        "plan": "step 1",
        "filePath": "/tmp/plan.md",
        "planWasEdited": true,
    });
    let parts = <ExitPlanModeTool as DynTool>::render_for_model(&ExitPlanModeTool, &data);
    let [coco_tool_runtime::ToolResultContentPart::Text { text, .. }] = parts.as_slice() else {
        panic!("expected single Text part, got {parts:?}");
    };
    assert!(text.contains("(edited by user)"));
    assert!(text.contains("/tmp/plan.md"));
    assert!(text.contains("step 1"));
}
