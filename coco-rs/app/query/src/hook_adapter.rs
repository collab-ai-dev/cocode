//! `QueryHookHandle` — bridges `coco_tool_runtime::HookHandle` to
//! `coco_hooks::orchestration`.
//!
//! The handle is installed into [`ToolUseContext`] by [`ToolContextFactory`]
//! (Phase 3 of the agent-loop refactor plan). Tools that ask for a
//! [`HookHandle`] get this implementation, which calls into the actual hook
//! registry without `coco-tool-runtime` having to depend on `coco-hooks`.
//!
//! # Hook outcome mapping
//!
//! | `AggregatedHookResult` field  | `HookHandle` outcome field               |
//! |------------------------------ |------------------------------------------|
//! | `updated_input`               | `PreToolUseOutcome::updated_input`       |
//! | `permission_behavior` Allow/Ask/Deny | `PreToolUseOutcome::permission_override` |
//! | `blocking_error`              | `blocking_reason`                        |
//! | `hook_permission_decision_reason` | `permission_reason`                  |
//! | `additional_contexts`         | `additional_contexts`                    |
//! | `system_message`              | `system_message`                         |
//! | `suppress_output`             | `suppress_output`                        |
//! | `updated_mcp_tool_output`     | `PostToolUseOutcome::updated_output` *(MCP path only; PostToolUse runner gates on `tool.is_mcp()`)* |
//! | `prevent_continuation`        | `prevent_continuation`                   |
//! | `stop_reason`                 | `stop_reason`                            |
//!
//! # Error handling
//!
//! Orchestration failures (hook timeout, hook process spawn failure, …) are
//! logged and **downgraded to a default (non-blocking) outcome**. Hook
//! infrastructure errors must not stop a tool call unless a hook itself
//! explicitly blocks.

use std::sync::Arc;

use async_trait::async_trait;
use coco_hooks::HookExecutionEvent;
use coco_hooks::HookRegistry;
use coco_hooks::orchestration;
use coco_hooks::orchestration::AggregatedHookResult;
use coco_hooks::orchestration::OrchestrationContext;
use coco_tool_runtime::HookHandle;
use coco_tool_runtime::HookPermission;
use coco_tool_runtime::PostToolUseOutcome;
use coco_tool_runtime::PreToolUseOutcome;
use coco_tool_runtime::TaskHookOutcome;
use coco_types::PermissionBehavior;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::warn;

/// Implementation of [`HookHandle`] backed by a real [`HookRegistry`].
///
/// Constructed once per session by [`QueryEngine`] and cloned into every
/// [`ToolUseContext`]. The handle is cheap to clone — it only holds
/// `Arc`/`Clone` fields.
pub(crate) struct QueryHookHandle {
    registry: Arc<HookRegistry>,
    ctx: OrchestrationContext,
    /// Optional channel for streaming hook lifecycle events to the
    /// `CoreEvent` forwarder. Shared with `coco-query`'s mid-turn
    /// orchestration so UI shows the same hook events whether they
    /// fire from the runner or the tool callback path.
    hook_tx: Option<mpsc::Sender<HookExecutionEvent>>,
}

impl QueryHookHandle {
    pub(crate) fn new(
        registry: Arc<HookRegistry>,
        ctx: OrchestrationContext,
        hook_tx: Option<mpsc::Sender<HookExecutionEvent>>,
    ) -> Self {
        Self {
            registry,
            ctx,
            hook_tx,
        }
    }
}

#[async_trait]
impl HookHandle for QueryHookHandle {
    async fn run_pre_tool_use(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        tool_input: &Value,
    ) -> PreToolUseOutcome {
        match orchestration::execute_pre_tool_use(
            &self.registry,
            &self.ctx,
            tool_name,
            tool_use_id,
            tool_input,
            self.hook_tx.as_ref(),
        )
        .await
        {
            Ok(agg) => aggregate_to_pre_outcome(&agg),
            Err(e) => {
                warn!(
                    error = %e,
                    tool = tool_name,
                    "PreToolUse hook orchestration failed; treating as non-blocking"
                );
                PreToolUseOutcome::default()
            }
        }
    }

    async fn run_post_tool_use(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        tool_input: &Value,
        tool_response: &Value,
    ) -> PostToolUseOutcome {
        match orchestration::execute_post_tool_use(
            &self.registry,
            &self.ctx,
            tool_name,
            tool_use_id,
            tool_input,
            tool_response,
            self.hook_tx.as_ref(),
        )
        .await
        {
            Ok(agg) => aggregate_to_post_outcome(&agg),
            Err(e) => {
                warn!(
                    error = %e,
                    tool = tool_name,
                    "PostToolUse hook orchestration failed; treating as non-blocking"
                );
                PostToolUseOutcome::default()
            }
        }
    }

    async fn run_post_tool_use_failure(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        tool_input: &Value,
        error_message: &str,
    ) -> PostToolUseOutcome {
        // `coco_hooks::orchestration::execute_post_tool_use_failure` is the
        // only structured wrapper — this is a distinct hook event and must
        // not reuse the PostToolUse path.
        match orchestration::execute_post_tool_use_failure(
            &self.registry,
            &self.ctx,
            tool_name,
            tool_use_id,
            tool_input,
            error_message,
            /*is_interrupt*/ None,
            self.hook_tx.as_ref(),
        )
        .await
        {
            Ok(agg) => aggregate_to_post_outcome(&agg),
            Err(e) => {
                warn!(
                    error = %e,
                    tool = tool_name,
                    "PostToolUseFailure hook orchestration failed; treating as non-blocking"
                );
                PostToolUseOutcome::default()
            }
        }
    }

    async fn run_task_created(
        &self,
        task_id: &str,
        task_subject: &str,
        task_description: Option<&str>,
        teammate_name: Option<&str>,
        team_name: Option<&str>,
    ) -> TaskHookOutcome {
        if self.ctx.disable_all_hooks {
            return TaskHookOutcome::default();
        }
        match orchestration::execute_task_created(
            &self.registry,
            &self.ctx,
            task_id,
            task_subject,
            task_description,
            teammate_name,
            team_name,
        )
        .await
        {
            Ok(agg) => TaskHookOutcome {
                blocking_reason: agg
                    .blocking_error
                    .as_ref()
                    .map(|e| e.blocking_error.clone()),
            },
            Err(e) => {
                warn!(error = %e, "TaskCreated hook orchestration failed; treating as non-blocking");
                TaskHookOutcome::default()
            }
        }
    }

    async fn run_task_completed(
        &self,
        task_id: &str,
        task_subject: &str,
        task_description: Option<&str>,
        teammate_name: Option<&str>,
        team_name: Option<&str>,
    ) -> TaskHookOutcome {
        if self.ctx.disable_all_hooks {
            return TaskHookOutcome::default();
        }
        match orchestration::execute_task_completed(
            &self.registry,
            &self.ctx,
            task_id,
            task_subject,
            task_description,
            teammate_name,
            team_name,
        )
        .await
        {
            Ok(agg) => TaskHookOutcome {
                blocking_reason: agg
                    .blocking_error
                    .as_ref()
                    .map(|e| e.blocking_error.clone()),
            },
            Err(e) => {
                warn!(error = %e, "TaskCompleted hook orchestration failed; treating as non-blocking");
                TaskHookOutcome::default()
            }
        }
    }
}

fn aggregate_to_pre_outcome(agg: &AggregatedHookResult) -> PreToolUseOutcome {
    PreToolUseOutcome {
        updated_input: agg.updated_input.clone(),
        permission_override: agg.permission_behavior.map(permission_behavior_to_override),
        blocking_reason: agg
            .blocking_error
            .as_ref()
            .map(|e| e.blocking_error.clone()),
        permission_reason: agg.hook_permission_decision_reason.clone(),
        additional_contexts: agg.additional_contexts.clone(),
        system_message: agg.system_message.clone(),
        suppress_output: agg.suppress_output,
    }
}

fn aggregate_to_post_outcome(agg: &AggregatedHookResult) -> PostToolUseOutcome {
    // `updated_output` is ONLY valid for MCP tools. The runner consuming
    // this outcome must gate substitution on `tool.is_mcp()` — carrying
    // the value through unconditionally still lets non-MCP runners observe
    // that a hook tried to rewrite output (for telemetry) while the runner
    // enforces the actual apply-or-ignore decision.
    PostToolUseOutcome {
        updated_output: agg.updated_mcp_tool_output.clone(),
        prevent_continuation: agg.prevent_continuation,
        stop_reason: agg.stop_reason.clone(),
        blocking_reason: agg
            .blocking_error
            .as_ref()
            .map(|e| e.blocking_error.clone()),
        additional_contexts: agg.additional_contexts.clone(),
        system_message: agg.system_message.clone(),
        suppress_output: agg.suppress_output,
    }
}

fn permission_behavior_to_override(behavior: PermissionBehavior) -> HookPermission {
    match behavior {
        PermissionBehavior::Allow => HookPermission::Allow,
        PermissionBehavior::Ask => HookPermission::Ask,
        PermissionBehavior::Deny => HookPermission::Deny,
    }
}

#[cfg(test)]
#[path = "hook_adapter.test.rs"]
mod tests;
