use std::sync::Arc;

use coco_hooks::HookExecutionEvent;
use coco_hooks::HookRegistry;
use coco_hooks::orchestration;
use coco_hooks::orchestration::AggregatedHookResult;
use coco_hooks::orchestration::OrchestrationContext;
use coco_llm_types::ToolCallPart;
use coco_messages::MessageHistory;
use coco_types::CoreEvent;
use coco_types::PermissionBehavior;
use coco_types::ToolId;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::warn;

use crate::helpers::ToolCompletionEventMode;
use crate::helpers::complete_tool_call_with_error_mode;

pub(crate) enum PreToolUseOutcome {
    Continue {
        updated_input: Option<Value>,
        permission_behavior: Option<PermissionBehavior>,
        reason: Option<String>,
    },
    Blocked,
}

#[derive(Default)]
pub(crate) struct PostToolUseOutcome {
    pub additional_contexts: Vec<String>,
    pub updated_mcp_tool_output: Option<Value>,
    pub prevent_continuation: bool,
    pub stop_reason: Option<String>,
}

#[derive(Default)]
pub(crate) struct PostToolUseFailureOutcome {
    pub additional_contexts: Vec<String>,
}

pub(crate) struct HookController<'a> {
    hooks: Option<&'a Arc<HookRegistry>>,
    ctx: OrchestrationContext,
    hook_tx: Option<&'a mpsc::Sender<HookExecutionEvent>>,
}

impl<'a> HookController<'a> {
    pub(crate) fn new(
        hooks: Option<&'a Arc<HookRegistry>>,
        ctx: OrchestrationContext,
        hook_tx: Option<&'a mpsc::Sender<HookExecutionEvent>>,
    ) -> Self {
        Self {
            hooks,
            ctx,
            hook_tx,
        }
    }

    pub(crate) async fn run_pre_tool_use(
        &self,
        event_tx: &Option<mpsc::Sender<CoreEvent>>,
        history: &mut MessageHistory,
        tool_call: &ToolCallPart,
        tool_id: &ToolId,
        completion_event_mode: ToolCompletionEventMode,
        deferred_tool_completions: Option<&mut crate::helpers::DeferredToolCompletionBuffer>,
    ) -> PreToolUseOutcome {
        let Some(hooks) = self.hooks else {
            return PreToolUseOutcome::Continue {
                updated_input: None,
                permission_behavior: None,
                reason: None,
            };
        };

        match orchestration::execute_pre_tool_use(
            hooks,
            &self.ctx,
            &tool_call.tool_name,
            &tool_call.tool_call_id,
            &tool_call.input,
            self.hook_tx,
        )
        .await
        {
            Ok(agg) if agg.is_blocked() => {
                let output = agg.blocking_error.as_ref().map_or_else(
                    || "PreToolUse hook blocked tool execution".to_string(),
                    |err| orchestration::format_pre_tool_blocking_message(err.command(), err),
                );
                warn!(
                    tool = tool_call.tool_name,
                    %output,
                    "PreToolUse hook blocked tool execution"
                );
                complete_tool_call_with_error_mode(
                    event_tx,
                    history,
                    &tool_call.tool_call_id,
                    &tool_call.tool_name,
                    tool_id,
                    &output,
                    coco_tool_runtime::ToolCallErrorKind::HookBlocked,
                    completion_event_mode,
                    deferred_tool_completions,
                )
                .await;
                PreToolUseOutcome::Blocked
            }
            Ok(agg) => PreToolUseOutcome::Continue {
                updated_input: agg.updated_input,
                permission_behavior: agg.permission_behavior,
                reason: agg.hook_permission_decision_reason,
            },
            Err(e) => {
                warn!(
                    error = %e,
                    tool = tool_call.tool_name,
                    "PreToolUse hook failed (non-blocking)"
                );
                PreToolUseOutcome::Continue {
                    updated_input: None,
                    permission_behavior: None,
                    reason: None,
                }
            }
        }
    }

    pub(crate) async fn run_post_tool_use(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        tool_input: &serde_json::Value,
        tool_response: &serde_json::Value,
    ) -> PostToolUseOutcome {
        let Some(hooks) = self.hooks else {
            return PostToolUseOutcome::default();
        };

        match orchestration::execute_post_tool_use(
            hooks,
            &self.ctx,
            tool_name,
            tool_use_id,
            tool_input,
            tool_response,
            self.hook_tx,
        )
        .await
        {
            Ok(agg) => self.build_post_tool_use_outcome(agg),
            Err(e) => {
                warn!(
                    error = %e,
                    tool = tool_name,
                    "PostToolUse hook failed (non-blocking)"
                );
                PostToolUseOutcome::default()
            }
        }
    }

    /// Fire `PermissionDenied` hooks.
    ///
    /// Called from `tool_call_preparer::prepare_one_pending_tool_call`
    /// when an auto-mode classifier denial is observed, before the
    /// `PermissionController` writes the error tool_result. Returns the
    /// aggregated `retry` flag so the caller can hint the model that the
    /// hook says it may retry.
    pub(crate) async fn run_permission_denied(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        tool_input: &serde_json::Value,
        reason: &str,
    ) -> bool {
        let Some(hooks) = self.hooks else {
            return false;
        };

        match orchestration::execute_permission_denied(
            hooks,
            &self.ctx,
            tool_name,
            tool_use_id,
            tool_input,
            reason,
        )
        .await
        {
            Ok(agg) => agg.retry,
            Err(e) => {
                warn!(
                    error = %e,
                    tool = tool_name,
                    "PermissionDenied hook failed (non-blocking)"
                );
                false
            }
        }
    }

    pub(crate) async fn run_post_tool_use_failure(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        tool_input: &serde_json::Value,
        error: &str,
        is_interrupt: bool,
    ) -> PostToolUseFailureOutcome {
        let Some(hooks) = self.hooks else {
            return PostToolUseFailureOutcome::default();
        };

        match orchestration::execute_post_tool_use_failure(
            hooks,
            &self.ctx,
            tool_name,
            tool_use_id,
            tool_input,
            error,
            Some(is_interrupt),
            self.hook_tx,
        )
        .await
        {
            Ok(agg) => PostToolUseFailureOutcome {
                additional_contexts: agg.additional_contexts,
            },
            Err(e) => {
                warn!(
                    error = %e,
                    tool = tool_name,
                    "PostToolUseFailure hook failed (non-blocking)"
                );
                PostToolUseFailureOutcome::default()
            }
        }
    }

    fn build_post_tool_use_outcome(&self, agg: AggregatedHookResult) -> PostToolUseOutcome {
        PostToolUseOutcome {
            additional_contexts: agg.additional_contexts,
            updated_mcp_tool_output: agg.updated_mcp_tool_output,
            prevent_continuation: agg.prevent_continuation,
            stop_reason: agg.stop_reason,
        }
    }
}
