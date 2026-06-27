//! Hook handle callback — the executor-visible interface for PreToolUse /
//! PostToolUse / PostToolUseFailure hook pipelines.
//!
//! # Architecture
//!
//! `coco-hooks` (root module) has the full hook execution machinery —
//! `HookRegistry`, `execute_pre_tool_use()`, `execute_post_tool_use()`,
//! `AggregatedHookResult`, permission aggregation, etc. But `coco-hooks`
//! sits *above* `coco-tool` in the layering, so `coco-tool` cannot depend
//! on it directly without inverting the dependency graph.
//!
//! Instead we use the **callback trait pattern** (same shape as `AgentHandle`
//! and `TaskHandle` in this crate):
//!
//! 1. `coco-tool` defines a thin `HookHandle` trait here with minimal DTO
//!    outcome types that live in `coco-tool` (no dep on `coco-hooks`).
//! 2. The higher-layer orchestrator (`app/query`) implements this trait by
//!    bridging to `coco_hooks::execute_pre_tool_use()` / `execute_post_tool_use()`
//!    and converting `AggregatedHookResult` → the DTO types below.
//! 3. `ToolExecutor` calls into the handle at the right lifecycle
//!    points, without ever touching `coco-hooks` types.
//!
//! # Lifecycle order
//!
//! ```text
//! validate_input → run_pre_tool_use (may override input/permission/reject)
//!                → check_permissions (unless hook overrode)
//!                → tool.execute()
//!                → run_post_tool_use (ok path) OR run_post_tool_use_failure (err path)
//! ```

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

/// Permission decision that a hook can emit to override the tool's own
/// `check_permissions()` result.
///
/// Aggregation rule: most-restrictive-wins.
/// `deny` always overrides `ask` which always overrides `allow`, which
/// overrides `passthrough` (absence). Passthrough means "hook has no
/// opinion, defer to the tool's own check".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookPermission {
    /// Auto-approve the tool call (skip user prompt).
    Allow,
    /// Force a user approval prompt regardless of the tool's own decision.
    Ask,
    /// Hard-deny the tool call. Tool will not execute.
    Deny,
}

/// Outcome of running all PreToolUse hooks for one tool call.
///
/// Each field is the already-aggregated value across all matching hooks —
/// callers don't see individual hook results.
#[derive(Debug, Clone, Default)]
pub struct PreToolUseOutcome {
    /// Input was rewritten by a `ModifyInput` hook. If `Some`, the executor
    /// must pass this value (not the original) to `tool.execute()`.
    pub updated_input: Option<Value>,

    /// Aggregated permission override. `None` means no hook voiced an
    /// opinion (passthrough) and the tool's own `check_permissions()`
    /// applies. See [`HookPermission`] for aggregation semantics.
    pub permission_override: Option<HookPermission>,

    /// Hard block reason. Set when any hook returned `Reject` — tool must
    /// not execute and the error is reported to the model.
    pub blocking_reason: Option<String>,

    /// Human-readable reason for the permission override (used for UI /
    /// telemetry). Independent from `blocking_reason`.
    pub permission_reason: Option<String>,

    /// Additional context lines to inject into the conversation before the
    /// tool's output is shown.
    pub additional_contexts: Vec<String>,

    /// Optional system message to surface to the user.
    pub system_message: Option<String>,

    /// When `true`, the tool's rendered output is hidden from the
    /// user-facing transcript display. The tool result still goes into
    /// the conversation history (so the model sees it), but the UI
    /// layer suppresses the normal rendering.
    /// Used for noisy or low-signal hooks that shouldn't clutter the user's view.
    pub suppress_output: bool,
}

impl PreToolUseOutcome {
    /// True iff the outcome forces a hard block (Reject or Deny override).
    pub fn is_blocked(&self) -> bool {
        self.blocking_reason.is_some()
            || matches!(self.permission_override, Some(HookPermission::Deny))
    }
}

/// Outcome of running all PostToolUse (or PostToolUseFailure) hooks for one
/// tool call.
///
/// Maps to a subset of `AggregatedHookResult`. The executor applies
/// `updated_output` by returning the replaced value in place of the original
/// tool result, and surfaces `prevent_continuation` to the agent loop to
/// optionally break out after this turn.
#[derive(Debug, Clone, Default)]
pub struct PostToolUseOutcome {
    /// Tool output was rewritten by a `ModifyOutput` hook. If `Some`, the
    /// executor must return this in place of the original tool result's
    /// data.
    pub updated_output: Option<Value>,

    /// The agent loop should stop after this tool call.
    pub prevent_continuation: bool,

    /// Reason text for `prevent_continuation` or blocking error.
    pub stop_reason: Option<String>,

    /// Hard block reason (post-hook `Reject` — replaces output with error).
    pub blocking_reason: Option<String>,

    /// Additional context to inject into the next user turn.
    pub additional_contexts: Vec<String>,

    /// Optional system message to surface to the user.
    pub system_message: Option<String>,

    /// When `true`, the tool's rendered output is hidden from the
    /// user-facing transcript display. See `PreToolUseOutcome::suppress_output`.
    pub suppress_output: bool,
}

impl PostToolUseOutcome {
    /// True iff a post-hook wants the loop to stop or hard-blocked the output.
    pub fn should_interrupt(&self) -> bool {
        self.prevent_continuation || self.blocking_reason.is_some()
    }
}

/// Outcome of running task lifecycle hooks (`TaskCreated`, `TaskCompleted`).
///
/// Mirrors the `AggregatedHookResult.blocking_error` semantic: a non-`None`
/// `blocking_reason` means a hook returned `decision: 'block'`. The caller
/// (TaskCreateTool / TaskUpdateTool) surfaces this to the model and rolls back
/// the operation.
#[derive(Debug, Clone, Default)]
pub struct TaskHookOutcome {
    /// Hard block reason. When set, the task operation must NOT proceed
    /// and the model must see the message.
    pub blocking_reason: Option<String>,
}

impl TaskHookOutcome {
    pub fn is_blocked(&self) -> bool {
        self.blocking_reason.is_some()
    }
}

/// Hook handle callback. Higher-layer orchestrators (e.g. `app/query`)
/// implement this by bridging to `coco-hooks::HookRegistry` +
/// `execute_pre_tool_use()` / `execute_post_tool_use()`.
///
/// All methods are async and must be cancellation-aware — the executor
/// passes `ctx.cancel_token()` transitively through its tool execution, and hook
/// execution should honor cancellation for long-running external hook
/// commands (default: 10 minute timeout per hook).
#[async_trait]
pub trait HookHandle: Send + Sync {
    /// Run PreToolUse hooks and return the aggregated outcome.
    ///
    /// The executor calls this AFTER input validation but BEFORE
    /// `check_permissions()` and `tool.execute()`. Hooks can:
    /// - rewrite the input (`updated_input`)
    /// - override permission to allow / ask / deny (`permission_override`)
    /// - hard-block the call with a reason (`blocking_reason`)
    /// - inject system messages or additional context
    async fn run_pre_tool_use(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        tool_input: &Value,
    ) -> PreToolUseOutcome;

    /// Run PostToolUse hooks on a successful tool result.
    ///
    /// Called AFTER `tool.execute()` returns `Ok`, BEFORE the result is
    /// yielded to the agent loop. Hooks can replace the output, prevent
    /// loop continuation, or inject context.
    async fn run_post_tool_use(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        tool_input: &Value,
        tool_response: &Value,
    ) -> PostToolUseOutcome;

    /// Run PostToolUseFailure hooks on a failed tool result.
    ///
    /// Called AFTER `tool.execute()` returns `Err`. Hooks can inject
    /// recovery context or prevent loop continuation.
    async fn run_post_tool_use_failure(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        tool_input: &Value,
        error_message: &str,
    ) -> PostToolUseOutcome;

    /// Run TaskCreated hooks before TaskCreateTool persists the task.
    ///
    /// Default impl is a no-op so existing test doubles don't need
    /// updating. The real `QueryHookHandle` overrides it.
    async fn run_task_created(
        &self,
        _task_id: &str,
        _task_subject: &str,
        _task_description: Option<&str>,
        _teammate_name: Option<&str>,
        _team_name: Option<&str>,
    ) -> TaskHookOutcome {
        TaskHookOutcome::default()
    }

    /// Run TaskCompleted hooks before TaskUpdateTool flips status to
    /// `completed`.
    async fn run_task_completed(
        &self,
        _task_id: &str,
        _task_subject: &str,
        _task_description: Option<&str>,
        _teammate_name: Option<&str>,
        _team_name: Option<&str>,
    ) -> TaskHookOutcome {
        TaskHookOutcome::default()
    }
}

pub type HookHandleRef = Arc<dyn HookHandle>;

/// No-op hook handle — for contexts without a configured hook registry
/// (e.g. unit tests, subagents that inherit empty registries). All methods
/// return default outcomes (no modifications, no blocks).
#[derive(Debug, Clone, Default)]
pub struct NoOpHookHandle;

#[async_trait]
impl HookHandle for NoOpHookHandle {
    async fn run_pre_tool_use(
        &self,
        _tool_name: &str,
        _tool_use_id: &str,
        _tool_input: &Value,
    ) -> PreToolUseOutcome {
        PreToolUseOutcome::default()
    }

    async fn run_post_tool_use(
        &self,
        _tool_name: &str,
        _tool_use_id: &str,
        _tool_input: &Value,
        _tool_response: &Value,
    ) -> PostToolUseOutcome {
        PostToolUseOutcome::default()
    }

    async fn run_post_tool_use_failure(
        &self,
        _tool_name: &str,
        _tool_use_id: &str,
        _tool_input: &Value,
        _error_message: &str,
    ) -> PostToolUseOutcome {
        PostToolUseOutcome::default()
    }
}

#[cfg(test)]
#[path = "hook_handle.test.rs"]
mod tests;
