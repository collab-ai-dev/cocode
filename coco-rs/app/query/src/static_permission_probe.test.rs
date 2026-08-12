use std::sync::Arc;

use coco_tool_runtime::ToolRegistry;
use coco_types::PermissionBehavior;
use coco_types::PermissionDecision;
use coco_types::PermissionMode;
use coco_types::PermissionProbeDynamicStage;
use coco_types::PermissionProbeValidation;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use serde_json::json;

use super::*;

fn context_with_tools() -> ToolUseContext {
    let registry = ToolRegistry::new();
    coco_tools::register_all_tools(&registry);
    let mut ctx = ToolUseContext::test_default();
    ctx.tools = Arc::new(registry);
    ctx.active_shell_tool = coco_types::ActiveShellTool::Bash;
    ctx.permission_context.mode = PermissionMode::Default;
    ctx
}

#[tokio::test]
async fn probe_reports_static_rule_decision_and_skipped_dynamic_stages() {
    let mut ctx = context_with_tools();
    ctx.permission_context.deny_rules.insert(
        PermissionRuleSource::Session,
        vec![PermissionRule {
            source: PermissionRuleSource::Session,
            behavior: PermissionBehavior::Deny,
            value: PermissionRuleValue {
                tool_pattern: "Bash".to_string(),
                rule_content: None,
            },
        }],
    );

    let result =
        probe_static_permission_with_context(&ctx, "Bash", json!({ "command": "printf hello" }))
            .await;

    assert!(
        matches!(result.validation, PermissionProbeValidation::Valid),
        "unexpected validation: {:?}",
        result.validation
    );
    assert!(matches!(
        result.provisional_decision,
        Some(PermissionDecision::Deny { .. })
    ));
    assert_eq!(
        result.not_evaluated,
        vec![
            PermissionProbeDynamicStage::PreToolUseHooks,
            PermissionProbeDynamicStage::CanUseTool,
            PermissionProbeDynamicStage::AutoModeClassifier,
            PermissionProbeDynamicStage::HumanApproval,
        ]
    );
}

#[tokio::test]
async fn probe_rejects_invalid_input_before_permission_checks() {
    let ctx = context_with_tools();
    let result = probe_static_permission_with_context(&ctx, "Bash", json!({})).await;

    assert!(
        matches!(result.validation, PermissionProbeValidation::Invalid { .. }),
        "unexpected validation: {:?}",
        result.validation
    );
    assert!(result.provisional_decision.is_none());
}

#[tokio::test]
async fn probe_reports_unknown_tool_without_guessing() {
    let ctx = context_with_tools();
    let result = probe_static_permission_with_context(&ctx, "Missing", json!({})).await;

    assert!(matches!(
        result.validation,
        PermissionProbeValidation::Unavailable { .. }
    ));
    assert!(result.provisional_decision.is_none());
}
