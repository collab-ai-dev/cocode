//! VerifyPlanExecutionTool — legacy post-plan verification checkpoint.
//!
//! Deprecated compatibility path. This tool does **not** verify anything
//! itself — the model is expected to inspect files and run checks first.
//! Calling the tool marks the TS-shaped `pending_plan_verification` state as
//! started and completed so the `verify_plan_reminder` nudge stops firing.
//!
//! The tool is intentionally not registered in the default tool registry while
//! the mirrored TS project conditionally imports it behind
//! `CLAUDE_CODE_VERIFY_PLAN`. The flow is inert unless
//! `settings.plan_mode.verify_execution` is explicitly enabled and this tool
//! is explicitly registered.

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_types::ToolCheckResult;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// Typed input for [`VerifyPlanExecutionTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct VerifyPlanExecutionInput {
    /// Brief summary of what was verified.
    #[serde(default)]
    pub summary: String,
    /// Any remaining issues or gaps found during verification.
    /// Leave empty when none.
    #[serde(default)]
    pub issues: String,
}

/// Status of the verification checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyPlanExecutionStatus {
    /// A pending plan verification was active and we recorded it.
    Verified,
    /// No pending verification flag was set — tool call was a no-op.
    NoPendingVerification,
}

/// Typed output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VerifyPlanExecutionOutput {
    pub status: VerifyPlanExecutionStatus,
    /// Absolute path to the plan file that backs this verification, if
    /// the session has one available (session id + plans dir resolved).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_file_path: Option<String>,
    pub summary: String,
    pub issues: String,
}

pub struct VerifyPlanExecutionTool;

#[async_trait::async_trait]
impl Tool for VerifyPlanExecutionTool {
    type Input = VerifyPlanExecutionInput;
    coco_tool_runtime::impl_runtime_schema!(VerifyPlanExecutionInput);
    type Output = VerifyPlanExecutionOutput;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::VerifyPlanExecution)
    }

    fn name(&self) -> &str {
        ToolName::VerifyPlanExecution.as_str()
    }

    fn description(
        &self,
        _input: &VerifyPlanExecutionInput,
        _options: &DescriptionOptions,
    ) -> String {
        "Record a checkpoint that you have verified the implementation against the approved plan. \
         This tool does not run any verification itself — do the verification first, then call it."
            .into()
    }

    async fn prompt(&self, _options: &PromptOptions) -> String {
        "Call this only AFTER you have directly verified that the implementation satisfies the \
         approved plan: inspect the relevant files and run the appropriate checks yourself (do \
         not delegate to the Agent tool or a subagent). The tool performs no verification of its \
         own — it just records the checkpoint and clears the pending plan-verification reminder."
            .into()
    }

    fn is_read_only(&self, _input: &VerifyPlanExecutionInput) -> bool {
        true
    }

    /// Record-only checkpoint with no input dependency — Plan mode can keep it visible.
    fn is_always_read_only(&self) -> bool {
        true
    }

    fn requires_user_interaction(&self) -> bool {
        false
    }

    async fn check_permissions(
        &self,
        _input: &VerifyPlanExecutionInput,
        _ctx: &ToolUseContext,
    ) -> ToolCheckResult {
        ToolCheckResult::Allow {
            updated_input: None,
            feedback: None,
        }
    }

    fn render_for_model(&self, out: &VerifyPlanExecutionOutput) -> Vec<ToolResultContentPart> {
        let mut text = match out.status {
            VerifyPlanExecutionStatus::NoPendingVerification => {
                "No pending plan verification was active.".to_string()
            }
            VerifyPlanExecutionStatus::Verified => {
                "Plan execution verification recorded.".to_string()
            }
        };
        if let Some(path) = out.plan_file_path.as_deref()
            && !path.is_empty()
        {
            text.push_str(&format!(" Plan file: {path}."));
        }
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: VerifyPlanExecutionInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<VerifyPlanExecutionOutput>, ToolError> {
        let has_open_pending = match ctx.app_state.as_ref() {
            Some(state) => state
                .read()
                .await
                .pending_plan_verification
                .as_ref()
                .is_some_and(|pending| !pending.verification_completed),
            None => false,
        };

        let session_id = ctx.session_id_for_history.as_deref();
        let agent_id = ctx.agent_id.as_ref().map(|a| a.as_str().to_string());
        let plan_file_path = match (session_id, ctx.plans_dir.as_ref()) {
            (Some(sid), Some(plans_dir)) => Some(
                coco_context::get_plan_file_path(sid, plans_dir, agent_id.as_deref())
                    .to_string_lossy()
                    .into_owned(),
            ),
            _ => None,
        };

        let status = if has_open_pending {
            VerifyPlanExecutionStatus::Verified
        } else {
            VerifyPlanExecutionStatus::NoPendingVerification
        };

        let patch: coco_types::AppStatePatch = Box::new(|state| {
            if let Some(pending) = state.pending_plan_verification.as_mut() {
                pending.verification_started = true;
                pending.verification_completed = true;
            }
        });

        Ok(ToolResult::data(VerifyPlanExecutionOutput {
            status,
            plan_file_path,
            summary: input.summary.trim().to_string(),
            issues: input.issues.trim().to_string(),
        })
        .with_patch(patch))
    }
}

#[cfg(test)]
#[path = "verify_plan_execution.test.rs"]
mod tests;
