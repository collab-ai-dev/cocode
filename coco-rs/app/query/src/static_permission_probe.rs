//! Side-effect-free permission analysis for diagnostics and programmatic
//! callers. This stops before hooks, fork policy, classifiers, approval, and
//! execution; the returned decision is therefore explicitly provisional.

use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::ValidatedInput;
use coco_types::PermissionProbeDynamicStage;
use coco_types::PermissionProbeValidation;
use coco_types::StaticPermissionProbeResult;
use serde_json::Value;

use crate::QueryEngine;

const DYNAMIC_STAGES: [PermissionProbeDynamicStage; 4] = [
    PermissionProbeDynamicStage::PreToolUseHooks,
    PermissionProbeDynamicStage::CanUseTool,
    PermissionProbeDynamicStage::AutoModeClassifier,
    PermissionProbeDynamicStage::HumanApproval,
];

/// Probe one tool call against a fully-built tool context without invoking any
/// effectful or externally-decided runtime stage.
pub async fn probe_static_permission_with_context(
    ctx: &ToolUseContext,
    tool_name: &str,
    input: Value,
) -> StaticPermissionProbeResult {
    let unavailable = |tool_id: String, message: String| StaticPermissionProbeResult {
        tool_id,
        validation: PermissionProbeValidation::Unavailable { message },
        normalized_input: None,
        provisional_decision: None,
        not_evaluated: DYNAMIC_STAGES.to_vec(),
    };

    let Some(registered) = ctx.tools.get_by_name(tool_name) else {
        return unavailable(
            tool_name.to_string(),
            format!("tool '{tool_name}' is not registered"),
        );
    };
    let tool_id = registered.id();
    if !registered.is_enabled(ctx)
        || !ctx.tool_overrides.permits(&tool_id)
        || !ctx.tool_filter.allows(&tool_id)
    {
        return unavailable(
            tool_id.to_string(),
            format!("tool '{tool_id}' is disabled or filtered in this context"),
        );
    }
    // Deferred placement is a model-prompt optimization, not a permission
    // property. Diagnostics may inspect any registered tool that passes the
    // actual feature/override/filter gates.
    let tool = registered;

    let validated = match ValidatedInput::validate(tool.as_ref(), input) {
        Ok(validated) => validated,
        Err(issues) => {
            return StaticPermissionProbeResult {
                tool_id: tool_id.to_string(),
                validation: PermissionProbeValidation::Invalid {
                    message: coco_tool_runtime::format_schema_error(tool_name, &issues),
                },
                normalized_input: None,
                provisional_decision: None,
                not_evaluated: DYNAMIC_STAGES.to_vec(),
            };
        }
    };
    let normalized_input = validated.as_value().clone();
    let typed_validation = tool.validate_input(&normalized_input, ctx);
    if let coco_tool_runtime::ValidationResult::Invalid { message, .. } = typed_validation {
        return StaticPermissionProbeResult {
            tool_id: tool_id.to_string(),
            validation: PermissionProbeValidation::Invalid { message },
            normalized_input: Some(normalized_input),
            provisional_decision: None,
            not_evaluated: DYNAMIC_STAGES.to_vec(),
        };
    }

    let decision =
        crate::tool_call_preparer::evaluate_with_rules(&tool, &normalized_input, None, ctx, false)
            .await;
    StaticPermissionProbeResult {
        tool_id: tool_id.to_string(),
        validation: PermissionProbeValidation::Valid,
        normalized_input: Some(normalized_input),
        provisional_decision: Some(decision),
        not_evaluated: DYNAMIC_STAGES.to_vec(),
    }
}

impl QueryEngine {
    /// Build the same live tool context used by execution and run a static
    /// permission probe against it. No hook, classifier, approval, or tool body
    /// is invoked.
    pub async fn probe_static_permission(
        &self,
        tool_name: &str,
        input: Value,
    ) -> StaticPermissionProbeResult {
        let ctx = self.build_base_tool_context().await;
        probe_static_permission_with_context(&ctx, tool_name, input).await
    }
}

#[cfg(test)]
#[path = "static_permission_probe.test.rs"]
mod tests;
