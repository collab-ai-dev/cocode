use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolUseContext;
use coco_types::Feature;
use coco_types::Features;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::ToolCheckResult;
use coco_types::ToolId;
use coco_types::ToolName;
use std::sync::Arc;

use super::WorkflowInput;
use super::WorkflowTool;

#[test]
fn workflow_tool_identity_and_alias_match_contract() {
    let tool = WorkflowTool;

    assert_eq!(tool.id(), ToolId::Builtin(ToolName::Workflow));
    assert_eq!(tool.name(), "Workflow");
    assert_eq!(tool.aliases(), ["RunWorkflow"]);
}

#[test]
fn workflow_tool_is_feature_gated() {
    let tool = WorkflowTool;
    let mut ctx = ToolUseContext::test_default();

    // Default-on.
    assert!(tool.is_enabled(&ctx));

    // Explicitly disabled → filtered out of the model's tool list.
    let mut features = Features::with_defaults();
    features.disable(Feature::Workflow);
    ctx.features = Arc::new(features);
    assert!(!tool.is_enabled(&ctx));
}

#[tokio::test]
async fn workflow_execute_without_task_runtime_errors() {
    // `test_default` has no `task_handle`, so a valid script cannot be
    // launched and the tool reports the missing background runtime.
    let tool = WorkflowTool;
    let ctx = ToolUseContext::test_default();
    let input = WorkflowInput {
        script: Some("export const meta = { name: 'x', description: 'y' };".into()),
        ..WorkflowInput::default()
    };

    let err = tool.execute(input, &ctx).await.unwrap_err();
    assert!(matches!(err, ToolError::ExecutionFailed { .. }));
    assert!(err.to_string().contains("Background tasks"));
}

#[tokio::test]
async fn workflow_execute_surfaces_source_errors_as_real_errors() {
    let tool = WorkflowTool;
    let ctx = ToolUseContext::test_default();
    let input = WorkflowInput {
        script: Some("const x = 1; export const meta = { name: 'x', description: 'y' };".into()),
        ..WorkflowInput::default()
    };

    // `meta` is not the first statement → parse failure surfaces as an error,
    // not a fake launch.
    let err = tool.execute(input, &ctx).await.unwrap_err();
    assert!(matches!(err, ToolError::ExecutionFailed { .. }));
    assert!(err.to_string().contains("not launched"));
}

#[test]
fn workflow_validate_input_accepts_ts_fields_and_rejects_bad_resume() {
    let tool = WorkflowTool;
    let ctx = ToolUseContext::test_default();
    let valid = WorkflowInput {
        script: Some("export const meta = { name: 'x', description: 'y' };".into()),
        description: Some("ignored".into()),
        title: Some("ignored".into()),
        args: serde_json::json!({"x": 1}),
        ..WorkflowInput::default()
    };

    assert!(tool.validate_input(&valid, &ctx).is_valid());

    let invalid = WorkflowInput {
        name: Some("x".into()),
        resume_from_run_id: Some("bad".into()),
        ..WorkflowInput::default()
    };
    assert!(!tool.validate_input(&invalid, &ctx).is_valid());
}

fn allow_rule(tool_pattern: &str, rule_content: Option<&str>) -> PermissionRule {
    PermissionRule {
        source: PermissionRuleSource::Session,
        behavior: coco_types::PermissionBehavior::Allow,
        value: PermissionRuleValue {
            tool_pattern: tool_pattern.to_string(),
            rule_content: rule_content.map(str::to_string),
        },
    }
}

#[tokio::test]
async fn workflow_named_allow_rule_allows_matching_name() {
    let tool = WorkflowTool;
    let mut ctx = ToolUseContext::test_default();
    ctx.permission_context.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![allow_rule("Workflow", Some("release"))],
    );

    let input = WorkflowInput {
        name: Some("release".into()),
        ..WorkflowInput::default()
    };
    assert!(matches!(
        tool.check_permissions(&input, &ctx).await,
        ToolCheckResult::Allow { .. }
    ));
}

#[tokio::test]
async fn workflow_script_path_with_name_rule_still_asks() {
    // A {scriptPath, name} call must never be auto-allowed by a Workflow(name)
    // rule — scriptPath has no stable identity, so it always asks.
    let tool = WorkflowTool;
    let mut ctx = ToolUseContext::test_default();
    ctx.permission_context.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![allow_rule("Workflow", Some("release"))],
    );

    let input = WorkflowInput {
        name: Some("release".into()),
        script_path: Some("/tmp/evil.js".into()),
        ..WorkflowInput::default()
    };
    assert!(matches!(
        tool.check_permissions(&input, &ctx).await,
        ToolCheckResult::Ask { .. }
    ));
}

#[tokio::test]
async fn workflow_inline_script_not_auto_allowed_by_contentless_rule() {
    // A content-less `Workflow` allow rule must not blanket-allow an inline
    // script (arbitrary code) — it falls through to Ask.
    let tool = WorkflowTool;
    let mut ctx = ToolUseContext::test_default();
    ctx.permission_context.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![allow_rule("Workflow", None)],
    );

    let input = WorkflowInput {
        script: Some("export const meta = { name: 'x', description: 'y' };".into()),
        ..WorkflowInput::default()
    };
    assert!(matches!(
        tool.check_permissions(&input, &ctx).await,
        ToolCheckResult::Ask { .. }
    ));
}
