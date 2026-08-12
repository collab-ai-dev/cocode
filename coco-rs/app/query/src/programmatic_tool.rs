//! Read-only programmatic calls from sandboxed runtimes.
//!
//! The handle owns a request-scoped materialization and re-enters the same
//! `ToolCallRunner` used for model-authored calls. It deliberately has no event
//! sender or shared history: nested calls retain hooks, permissions,
//! cancellation, accounting, and tracing without creating phantom transcript
//! entries in the parent session.

use std::sync::Arc;

use coco_event_types::PermissionDenialInfo;
use coco_hooks::HookRegistry;
use coco_hooks::orchestration::OrchestrationContext;
use coco_inference::ModelRuntimeRegistry;
use coco_llm_types::LlmMessage;
use coco_llm_types::ToolCallPart;
use coco_llm_types::ToolContentPart;
use coco_llm_types::ToolResultContent;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_permissions::AutoModeRules;
use coco_tool_runtime::MaterializedToolLookup;
use coco_tool_runtime::ProgrammaticToolCallError;
use coco_tool_runtime::ProgrammaticToolCallHandle;
use coco_tool_runtime::ProgrammaticToolCallHandleRef;
use coco_tool_runtime::ToolMaterialization;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::ValidatedInput;
use coco_types::SessionId;
use coco_types::ToolAppState;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::QueryEngine;
use crate::session_state::SessionStateTracker;
use crate::tool_call_runner::ToolCallRunner;

struct QueryProgrammaticToolHandle {
    ctx: ToolUseContext,
    tools: Arc<ToolRegistry>,
    materialization: Arc<ToolMaterialization>,
    hooks: Option<Arc<HookRegistry>>,
    hook_execution_policy: coco_hooks::HookExecutionPolicy,
    orchestration_ctx: OrchestrationContext,
    permission_bridge: Option<ToolPermissionBridgeRef>,
    session_id: SessionId,
    cancel: CancellationToken,
    auto_mode_state: Option<Arc<coco_permissions::AutoModeState>>,
    denial_tracker: Option<Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>>>,
    model_runtimes: Arc<ModelRuntimeRegistry>,
    usage_accounting: Option<crate::usage_accounting::UsageAccounting>,
    auto_mode_rules: AutoModeRules,
    app_state: Option<Arc<RwLock<ToolAppState>>>,
    permission_rule_handle: coco_tool_runtime::PermissionRuleHandleRef,
}

impl QueryEngine {
    pub(crate) fn programmatic_tool_handle(
        &self,
        mut ctx: ToolUseContext,
        materialization: Arc<ToolMaterialization>,
    ) -> ProgrammaticToolCallHandleRef {
        ctx.avoid_permission_prompts = true;
        ctx.require_read_only = true;
        // Do not let a nested read-only tool recursively acquire this bridge.
        ctx.programmatic_tools = None;
        Arc::new(QueryProgrammaticToolHandle {
            ctx,
            tools: self.tools.clone(),
            materialization,
            hooks: self.hooks.clone(),
            hook_execution_policy: self.hook_execution_policy,
            orchestration_ctx: self.orchestration_ctx(),
            permission_bridge: self.permission_bridge.clone(),
            session_id: self.session_id.clone(),
            cancel: self.cancel.clone(),
            auto_mode_state: self.auto_mode_state.clone(),
            denial_tracker: self.denial_tracker.clone(),
            model_runtimes: self.model_runtimes.clone(),
            usage_accounting: self.usage_accounting.clone(),
            auto_mode_rules: self.auto_mode_rules.clone(),
            app_state: self.app_state.clone(),
            permission_rule_handle: self.permission_rule_handle.clone(),
        })
    }
}

#[async_trait::async_trait]
impl ProgrammaticToolCallHandle for QueryProgrammaticToolHandle {
    async fn call_read_only(
        &self,
        tool_name: String,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, ProgrammaticToolCallError> {
        let unavailable = |reason| ProgrammaticToolCallError::Unavailable {
            tool: tool_name.clone(),
            reason,
        };
        let registered = self
            .tools
            .get_by_name(&tool_name)
            .ok_or_else(|| unavailable("tool is not registered".to_string()))?;
        let materialized = match self.materialization.lookup(&self.tools, &registered.id()) {
            MaterializedToolLookup::Loaded(tool) => tool,
            MaterializedToolLookup::Deferred { .. } => {
                return Err(unavailable("tool is deferred in this request".to_string()));
            }
            MaterializedToolLookup::Stale { .. } => {
                return Err(unavailable("tool registration changed".to_string()));
            }
            MaterializedToolLookup::Unavailable => {
                return Err(unavailable("tool is disabled or filtered".to_string()));
            }
        };

        let validated =
            ValidatedInput::validate(materialized.tool.as_ref(), input).map_err(|issues| {
                ProgrammaticToolCallError::InvalidInput {
                    tool: tool_name.clone(),
                    reason: coco_tool_runtime::format_schema_error(&tool_name, &issues),
                }
            })?;
        if let coco_tool_runtime::ValidationResult::Invalid { message, .. } = materialized
            .tool
            .validate_input(validated.as_value(), &self.ctx)
        {
            return Err(ProgrammaticToolCallError::InvalidInput {
                tool: tool_name,
                reason: message,
            });
        }
        if !crate::tool_call_preparer::is_dynamic_read_only(
            &materialized.tool,
            &materialized.tool_id,
            validated.as_value(),
            &self.ctx,
        )
        .await
        {
            return Err(ProgrammaticToolCallError::NotReadOnly { tool: tool_name });
        }

        let call_id = format!("programmatic_{}", uuid::Uuid::new_v4());
        let call = ToolCallPart::new(
            call_id.clone(),
            materialized.wire_name.as_str(),
            validated.into_value(),
        );
        let mut history =
            MessageHistory::from_arcs_preserving_latest_usage(self.ctx.messages.as_ref().clone());
        let event_tx = None;
        let mut permission_denials: Vec<PermissionDenialInfo> = Vec::new();
        let state_tracker = SessionStateTracker::new();
        let outcome = ToolCallRunner {
            event_tx: &event_tx,
            history: &mut history,
            ctx: &self.ctx,
            tool_calls: std::slice::from_ref(&call),
            turn: 0,
            tools: &self.tools,
            tool_materialization: &self.materialization,
            hooks: self.hooks.as_ref(),
            hook_execution_policy: self.hook_execution_policy,
            orchestration_ctx: self.orchestration_ctx.clone(),
            hook_tx_opt: None,
            permission_denials: &mut permission_denials,
            state_tracker: &state_tracker,
            permission_bridge: self.permission_bridge.as_ref(),
            session_id: &self.session_id,
            cancel: &self.cancel,
            auto_mode_state: self.auto_mode_state.as_ref(),
            denial_tracker: self.denial_tracker.as_ref(),
            model_runtimes: &self.model_runtimes,
            usage_accounting: self.usage_accounting.as_ref(),
            auto_mode_rules: &self.auto_mode_rules,
            app_state: self.app_state.as_ref(),
            permission_rule_handle: &self.permission_rule_handle,
        }
        .run()
        .await;

        if let Some(output) = outcome.programmatic_output {
            return Ok(output);
        }

        extract_result(&history, &call_id).ok_or_else(|| ProgrammaticToolCallError::Failed {
            tool: tool_name,
            reason: "canonical runner produced no tool result".to_string(),
        })?
    }
}

fn extract_result(
    history: &MessageHistory,
    call_id: &str,
) -> Option<Result<serde_json::Value, ProgrammaticToolCallError>> {
    history.iter().rev().find_map(|message| {
        let Message::ToolResult(result) = message.as_ref() else {
            return None;
        };
        if result.tool_use_id != call_id {
            return None;
        }
        let LlmMessage::Tool { content, .. } = &result.message else {
            return Some(Err(ProgrammaticToolCallError::Failed {
                tool: result.tool_id.to_string(),
                reason: "tool result had an invalid message role".to_string(),
            }));
        };
        let part = content.iter().find_map(|part| match part {
            ToolContentPart::ToolResult(part) if part.tool_call_id == call_id => Some(part),
            _ => None,
        })?;
        let failure = |reason| {
            Err(ProgrammaticToolCallError::Failed {
                tool: result.tool_id.to_string(),
                reason,
            })
        };
        Some(match &part.output {
            ToolResultContent::Text { value, .. } if !result.is_error && !part.is_error => {
                Ok(serde_json::Value::String(value.clone()))
            }
            ToolResultContent::Json { value, .. } if !result.is_error && !part.is_error => {
                Ok(value.clone())
            }
            ToolResultContent::Text { value, .. } | ToolResultContent::ErrorText { value, .. } => {
                failure(value.clone())
            }
            ToolResultContent::Json { value, .. } | ToolResultContent::ErrorJson { value, .. } => {
                failure(value.to_string())
            }
            ToolResultContent::ExecutionDenied { reason, .. } => failure(
                reason
                    .clone()
                    .unwrap_or_else(|| "tool execution was denied".to_string()),
            ),
            ToolResultContent::Content { value, .. } if !result.is_error && !part.is_error => {
                serde_json::to_value(value).map_err(|error| ProgrammaticToolCallError::Failed {
                    tool: result.tool_id.to_string(),
                    reason: format!("could not encode tool content: {error}"),
                })
            }
            ToolResultContent::Content { value, .. } => failure(
                serde_json::to_string(value)
                    .unwrap_or_else(|_| "tool returned error content".to_string()),
            ),
        })
    })
}

#[cfg(test)]
#[path = "programmatic_tool.test.rs"]
mod tests;
