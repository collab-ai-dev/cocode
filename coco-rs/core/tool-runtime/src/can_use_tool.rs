//! Per-fork tool-execution gate.
//!
//!
//! ## Why a callback at the per-call gate
//!
//! Forks need to gate tool execution dynamically based on per-call
//! state (file path, mode, overlay membership) — a static
//! allow/deny rule list cannot express speculation's "rewrite
//! `file_path` to overlay_path" semantics or auto-mem's "Edit only
//! when path is under `memory_dir`".
//!
//! The handle is invoked by the app/query tool-call preparer
//! (`resolve_can_use_tool_decision`) before the static permission
//! evaluator consults `tool.check_permissions`. Decision variants:
//!
//! - [`CanUseToolDecision::Deny`] short-circuits with the message
//!   surfaced as the `tool_result` content.
//! - [`CanUseToolDecision::Allow`] with `updated_input: Some(...)`
//!   rewrites the value passed to permissions AND execute. This is
//!   the path-rewrite hook speculation overlay needs.
//! - [`CanUseToolDecision::Allow`] with `updated_input: None`
//!   proceeds with the original input but skips the tool's
//!   built-in `check_permissions` (the callback's opinion is final).
//! - [`CanUseToolDecision::Ask`] falls through to the tool's
//!   built-in `check_permissions` ("callback abstains" case).
//!
//! ## `requireCanUseTool` interaction with hook auto-approve
//!
//! When [`CanUseToolCallContext::require_can_use_tool`] is `true`,
//! a `Pre`-tool-use hook that auto-approved cannot bypass the
//! callback — speculation needs this so file-system overlay
//! redirects always run, regardless of hook config.
//!
//! ## NoOp baseline
//!
//! [`NoOpCanUseToolHandle`] returns `Ask` for every call so non-fork
//! code paths see no behavior change after step 3.5 lands.

use std::sync::Arc;

use async_trait::async_trait;
use coco_messages::Message;
use serde_json::Value;

use crate::cancellation::TurnAbortSignal;

/// Reason field accompanying a [`CanUseToolDecision`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionReason {
    /// Free-form. Use when no other variant fits — caller-provided
    /// label surfaces in telemetry / transcripts unchanged.
    Other { reason: String },
    /// A configured permission rule allowed the call.
    RuleAllow { rule_kind: String },
    /// A configured permission rule denied the call.
    RuleDeny { rule_kind: String },
    /// `permission_mode == AcceptEdits | BypassPermissions` short-circuited.
    ModeAllow,
    /// User clicked "Allow" in the permission dialog.
    UserAccept,
    /// User clicked "Deny" in the permission dialog.
    UserReject,
    /// Speculation 3-boundary classification — see
    /// [`SpeculationBoundary`] for the discriminator.
    Speculation { boundary: SpeculationBoundary },
}

/// Why a speculation fork stopped (or which boundary caused a deny).
///
/// Carried on the `Speculation` variant of [`DecisionReason`] so the
/// telemetry / TUI toast can attribute the boundary precisely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeculationBoundary {
    /// Edit / Write / NotebookEdit attempted in a mode that doesn't
    /// allow overlay writes.
    Edit,
    /// Bash command failed `is_known_safe_command` (mutating).
    Bash,
    /// Tool not in `WRITE_TOOLS | SAFE_READ_ONLY_TOOLS | {Bash}`.
    DeniedTool,
}

/// Decision returned by a [`CanUseToolHandle::check`] call.
///
/// Shape: `{ behavior: 'allow' | 'deny' | 'ask', updatedInput?,
/// decisionReason, message? }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanUseToolDecision {
    /// Proceed to execute. When `updated_input` is `Some(v)`, `v`
    /// replaces the original input passed to permissions + execute
    /// (speculation overlay path-rewrite hook).
    ///
    /// In both `Some` and `None` cases the tool's built-in
    /// `check_permissions` is **skipped** — the callback is
    /// authoritative for the Allow path.
    Allow {
        updated_input: Option<Value>,
        decision_reason: DecisionReason,
    },
    /// Short-circuit with `message` as the synthesized
    /// `tool_result` content. The agent loop sees a denial, the
    /// model can choose to retry with different args or give up.
    Deny {
        message: String,
        decision_reason: DecisionReason,
    },
    /// Callback abstains — fall through to the tool's built-in
    /// `check_permissions`. Use this when the callback only cares
    /// about a subset of tools (e.g. session-mem callback only
    /// cares about `Edit`).
    Ask { decision_reason: DecisionReason },
}

/// Per-call context surfaced to the callback. Distinct from
/// [`crate::context::ToolUseContext`] so the callback can't
/// accidentally mutate engine state — read-only handles only.
#[derive(Clone)]
pub struct CanUseToolCallContext {
    /// The model-emitted tool_use_id for this call.
    pub tool_use_id: String,
    /// Turn abort signal; the callback should respect it for any async work
    /// (e.g. fs lookups) and may inspect the structured reason.
    pub abort: TurnAbortSignal,
    /// When `true`, hook auto-approve cannot bypass the callback.
    /// Speculation needs this so overlay path-rewriting always
    /// runs regardless of hook config.
    pub require_can_use_tool: bool,
    /// Read-only post-budget message snapshot at call time. Used by
    /// callbacks that need to inspect prior context (e.g. session-mem
    /// can check that the parent transcript contains the memory file
    /// path). Shares allocations with `ToolUseContext.messages` —
    /// same `Arc<Vec<Arc<Message>>>` instance.
    pub messages: Arc<Vec<Arc<Message>>>,
}

impl std::fmt::Debug for CanUseToolCallContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanUseToolCallContext")
            .field("tool_use_id", &self.tool_use_id)
            .field("require_can_use_tool", &self.require_can_use_tool)
            .field("abort_cancelled", &self.abort.is_aborted())
            .finish_non_exhaustive()
    }
}

/// Per-fork tool-execution gate.
///
/// Production implementations live with the calling subsystem:
/// - `coco-memory` provides `create_auto_mem_handle` and
///   `create_session_mem_handle`.
/// - `coco-query::speculation` provides `create_speculation_handle`.
/// - `deny_all_handle` lives here for the four deny-all callers
///   (prompt_suggestion / side_question / compact / agent_summary).
///
/// All implementations must be `Send + Sync` because the executor's
/// `clone_for_concurrent` shares the `Arc` across worker tasks.
///
/// `Debug` is a supertrait so structs that derive Debug (e.g.
/// `AgentSpawnRequest`) can carry an `Option<CanUseToolHandleRef>`.
/// Implementations should print only their type name + reason
/// (don't leak callback closures into log output).
#[async_trait]
pub trait CanUseToolHandle: std::fmt::Debug + Send + Sync {
    /// Decide whether the tool call may proceed.
    ///
    /// Implementations MUST honor `ctx.abort` for any async work
    /// they do — the executor passes the parent's cancellation
    /// token through.
    async fn check(
        &self,
        tool_name: &str,
        input: &Value,
        ctx: &CanUseToolCallContext,
    ) -> CanUseToolDecision;
}

/// `Arc`-shareable reference to a [`CanUseToolHandle`].
pub type CanUseToolHandleRef = Arc<dyn CanUseToolHandle>;

/// No-op handle — every call returns `Ask`, falling through to the
/// tool's built-in `check_permissions`. Used by non-fork code
/// paths so they see no behavior change after step 3.5 lands.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpCanUseToolHandle;

#[async_trait]
impl CanUseToolHandle for NoOpCanUseToolHandle {
    async fn check(
        &self,
        _tool_name: &str,
        _input: &Value,
        _ctx: &CanUseToolCallContext,
    ) -> CanUseToolDecision {
        CanUseToolDecision::Ask {
            decision_reason: DecisionReason::Other {
                reason: "no-op handle".into(),
            },
        }
    }
}

/// Deny every tool call. Used by `prompt_suggestion`, `side_question`,
/// `compact`, `agent_summary` — text-only forks where any tool
/// invocation is wasteful.
///
/// `reason` surfaces in `DecisionReason::Other` for telemetry.
pub fn deny_all_handle(reason: &'static str) -> CanUseToolHandleRef {
    Arc::new(DenyAllHandle { reason })
}

#[derive(Debug)]
struct DenyAllHandle {
    reason: &'static str,
}

#[async_trait]
impl CanUseToolHandle for DenyAllHandle {
    async fn check(
        &self,
        _tool_name: &str,
        _input: &Value,
        _ctx: &CanUseToolCallContext,
    ) -> CanUseToolDecision {
        CanUseToolDecision::Deny {
            message: format!("tool denied: {}", self.reason),
            decision_reason: DecisionReason::Other {
                reason: self.reason.into(),
            },
        }
    }
}

#[cfg(test)]
#[path = "can_use_tool.test.rs"]
mod tests;
