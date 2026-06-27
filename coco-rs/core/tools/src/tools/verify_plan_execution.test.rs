use super::VerifyPlanExecutionTool;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::ToolUseContext;
use coco_types::PendingPlanVerificationState;
use coco_types::PermissionMode;
use coco_types::ToolAppState;
use coco_types::ToolName;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

fn text(result: &coco_messages::ToolResult<serde_json::Value>) -> String {
    match <VerifyPlanExecutionTool as DynTool>::render_for_model(
        &VerifyPlanExecutionTool,
        &result.data,
    )
    .as_slice()
    {
        [coco_tool_runtime::ToolResultContentPart::Text { text, .. }] => text.clone(),
        _ => panic!("expected single text result"),
    }
}

#[tokio::test]
async fn execute_marks_pending_plan_verification_completed() {
    let app_state = Arc::new(RwLock::new(ToolAppState {
        pending_plan_verification: Some(PendingPlanVerificationState::new("# plan".into())),
        ..ToolAppState::default()
    }));
    let mut ctx = ToolUseContext::test_default();
    ctx.app_state = Some(app_state.clone().into());
    ctx.session_id_for_history = Some("session-1".into());
    ctx.plans_dir = Some(std::env::temp_dir());

    let mut result = <VerifyPlanExecutionTool as DynTool>::execute(
        &VerifyPlanExecutionTool,
        json!({"summary": "checked files and tests", "issues": ""}),
        &ctx,
    )
    .await
    .unwrap();
    let patch = result
        .app_state_patch
        .take()
        .expect("tool records pending verification");
    {
        let mut guard = app_state.write().await;
        patch(&mut guard);
    }

    let pending = app_state
        .read()
        .await
        .pending_plan_verification
        .clone()
        .expect("pending verification state is retained");
    assert!(pending.verification_started);
    assert!(pending.verification_completed);
    assert_eq!(result.data["status"], "verified");
    assert_eq!(result.data["summary"], "checked files and tests");
    assert!(text(&result).contains("Plan execution verification recorded."));
}

#[tokio::test]
async fn execute_is_idempotent_without_pending_verification() {
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));
    let mut ctx = ToolUseContext::test_default();
    ctx.app_state = Some(app_state.clone().into());

    let result =
        <VerifyPlanExecutionTool as DynTool>::execute(&VerifyPlanExecutionTool, json!({}), &ctx)
            .await
            .unwrap();

    assert_eq!(result.data["status"], "no_pending_verification");
    assert!(text(&result).contains("No pending plan verification"));
}

#[tokio::test]
async fn check_permissions_allows_without_prompt() {
    let mut ctx = ToolUseContext::test_default();
    ctx.permission_context.mode = PermissionMode::Default;

    let decision = <VerifyPlanExecutionTool as DynTool>::check_permissions(
        &VerifyPlanExecutionTool,
        &json!({}),
        &ctx,
    )
    .await;

    match decision {
        coco_types::ToolCheckResult::Allow {
            updated_input,
            feedback,
        } => {
            assert!(updated_input.is_none());
            assert!(feedback.is_none());
        }
        other => panic!("expected allow, got {other:?}"),
    }
}

#[test]
fn identity_and_schema() {
    assert_eq!(
        <VerifyPlanExecutionTool as DynTool>::name(&VerifyPlanExecutionTool,),
        ToolName::VerifyPlanExecution.as_str()
    );
    assert!(
        <VerifyPlanExecutionTool as DynTool>::runtime_validation_schema(&VerifyPlanExecutionTool)
            .as_value()["properties"]
            .get("summary")
            .is_some()
    );
}
