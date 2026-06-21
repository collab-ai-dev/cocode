//! Scheduler DTOs for the `ToolCallRunner` / `ToolExecutor`
//! boundary.
//!
//! I12 of `docs/coco-rs/agent-loop-refactor-plan.md`. The
//! runner returns an **unstamped** outcome carrying everything except
//! `completion_seq`; the executor stamps the completion sequence at
//! surface time — the moment the `run_one` future resolves for a
//! `Runnable` plan, or when the partitioner reaches an `EarlyOutcome`
//! barrier block.
//!
//! Keeping the DTOs in `coco-tool` (L3, the interface crate) rather
//! than `coco-query` (L5) preserves the dependency direction: the
//! executor lives in `coco-tool`, so it must not reference `coco-query`
//! types. `coco-query` owns the `ToolCallRunner` implementation and
//! the message-bucket helpers; the executor schedules and stamps.
//!
use coco_messages::Message;
use coco_types::AppStatePatch;
use coco_types::PermissionDenialInfo;
use coco_types::PermissionResolutionDetail;
use coco_types::PermissionUpdate;
use coco_types::ToolId;
use std::sync::Arc;

use crate::cancellation::ToolAbortSignal;
use crate::traits::DynTool;

/// What [`prepare_batch`](crate::call_plan) returns for each committed
/// assistant tool-use entry.
///
/// `EarlyOutcome` carries an **unstamped** outcome body — the
/// executor stamps `completion_seq` when it reaches that plan's
/// barrier block in partition order. `prepare_batch` does not know the
/// completion order yet.
///
/// (See "JSON Parse Failures Are Pre-Commit, Not Pre-Batch" in the
/// plan — parse failures are dropped in the streaming accumulator
/// before a committed tool_use exists, so no `InvalidJson` variant is
/// needed here.)
pub enum ToolCallPlan {
    /// Tool resolved, schema-validated. Ready for `run_one`.
    Runnable(PreparedToolCall),
    /// Preparation failed (unknown tool, schema failure, or another
    /// pre-execution gate decided after the assistant message
    /// committed). The outcome is unstamped; the executor stamps
    /// `completion_seq` and surfaces it to `on_outcome`.
    EarlyOutcome(UnstampedToolCallOutcome),
}

/// Effect-free, context-free preparation only.
///
/// Stores the resolved tool, the original invocation, and the
/// schema-validated input. Does **not** store `ToolUseContext`, hook
/// results, permission decisions, or `tool.validate_input()` results —
/// those depend on `&ToolUseContext` and must be computed serially
/// during execution (see I3 + the plan's "Scheduling Contract"). See
/// `prepare_batch` for why batched semantic prep is unsafe for serial
/// tools.
pub struct PreparedToolCall {
    /// Model-visible tool-call id.
    pub tool_use_id: String,
    /// Canonical tool identity resolved at `prepare_batch` time.
    /// Downstream uses this for permission / hook / audit / event
    /// routing — never re-parses a name string.
    pub tool_id: ToolId,
    /// The resolved tool. The runner already holds an `Arc`, so this
    /// is the single cheap clone across the scheduler boundary.
    pub tool: Arc<dyn DynTool>,
    /// Coerced, schema-validated model input ([`crate::ValidatedInput`] by
    /// construction). Hook rewrites during `run_one` may replace this with a
    /// re-validated `updated_input` before permission / execution.
    pub parsed_input: crate::ValidatedInput,
    /// Context-aware concurrency decision computed by the caller that holds
    /// the current `ToolUseContext`.
    pub is_concurrency_safe: bool,
    /// Tool-use position within the assistant message. Used to
    /// address per-call hook/permission state and to order
    /// `app_state_patch` application within a concurrent-safe batch.
    /// **Not** a history-append slot — that is `completion_seq`,
    /// stamped by the executor.
    pub model_index: usize,
    /// Trusted per-call approval metadata captured during permission
    /// resolution (e.g. the `ExitPlanMode` choice). The streaming feed
    /// populates this from the resolved `ToolResultContext` so the
    /// streaming run-closure can thread it into the execute-time
    /// `ToolUseContext`, matching the batch runner. `None` when the call
    /// carries no approval detail (the common case).
    pub permission_resolution_detail: Option<PermissionResolutionDetail>,
    /// User feedback captured alongside an approval (e.g. plan-approval
    /// notes). Threaded into `ToolUseContext.approval_feedback`.
    pub approval_feedback: Option<String>,
}

/// Scheduler-owned per-tool runtime.
///
/// Built by the executor and handed to the runner via the `run_one`
/// callback. Keeps scheduler state (cancellation tokens, sibling-abort
/// broadcasts, `model_index`) out of the runner's semantic surface while
/// letting `run_one` forward them into the `ToolUseContext` it builds per
/// call. The execute-time `ToolUseContext.progress_tx` is inherited from
/// the base ctx via `clone_for_tool_call` — it is not threaded here.
pub struct RunOneRuntime {
    /// Structured per-tool abort signal combining turn, self, and sibling
    /// abort sources.
    pub abort: ToolAbortSignal,
    /// Echoes `PreparedToolCall.model_index` so the runner can tag
    /// patches and telemetry without a separate lookup.
    pub model_index: usize,
}

/// Runner output before the executor stamps `completion_seq`.
///
/// Carries every field of the final outcome except the completion
/// sequence. Constructable without knowing the turn-wide completion
/// order — only the executor can assign it.
///
/// Split from [`ToolCallOutcome`] so "completion_seq is assigned by
/// the executor" is a type-system guarantee, not a documentation
/// convention. The executor calls
/// [`UnstampedToolCallOutcome::stamp_and_extract_effects`] to
/// produce the final outcome plus the scheduler-facing
/// [`ToolSideEffects`].
pub struct UnstampedToolCallOutcome {
    pub tool_use_id: String,
    pub tool_id: ToolId,
    pub model_index: usize,
    /// Pre-flattened message stream in model index order. The runner has
    /// already resolved the MCP / non-MCP + Success / Failure /
    /// EarlyReturn template while it still held the `Arc<dyn DynTool>`,
    /// so `QueryEngine` appends this verbatim.
    pub ordered_messages: Vec<Message>,
    /// Which lifecycle path produced `ordered_messages`. Retained for
    /// telemetry and tests; consumers must not re-derive message
    /// order from this field.
    pub message_path: ToolMessagePath,
    /// Optional synthetic-error classification for EarlyReturn /
    /// Failure paths. `None` on Success.
    pub error_kind: Option<ToolCallErrorKind>,
    /// Populated on permission denial so `QueryResult.permission_denials`
    /// accumulates audit entries.
    pub permission_denial: Option<PermissionDenialInfo>,
    /// Success-path `prevent_continuation` marker. Always `None` on
    /// Failure / EarlyReturn — the flatten template already rejects
    /// prevent attachments there.
    pub prevent_continuation: Option<String>,
    /// SDK-facing structured output, set when the tool returned
    /// `ToolResult::with_structured_output(...)`. Carries the typed
    /// payload back to `run_session_loop` so the SDK result branch in
    /// `make_query_result` can pick it up without scanning history
    /// (which is unsound under mid-query compaction).
    pub structured_output: Option<serde_json::Value>,
    /// Scheduler-facing side-effects. Separated from the history-facing
    /// outcome body because `AppStatePatch` is a single owned `FnOnce`
    /// that cannot simultaneously ride with the outcome into
    /// `on_outcome` (history) AND stay with the executor for later
    /// `apply`. `stamp_and_extract_effects` splits them: the executor
    /// keeps `ToolSideEffects`; the history-facing `ToolCallOutcome`
    /// is patch-free by construction.
    pub effects: ToolSideEffects,
}

/// Scheduler-facing side-effects moved out of the outcome body at
/// surface time.
///
/// Discarded (effects dropped, never applied) on error per I9 —
/// `Drop` runs the `FnOnce` destructor without invoking it.
pub struct ToolSideEffects {
    /// Mutation to apply to shared `ToolAppState`. Serial tools apply
    /// before the next tool's `ToolUseContext` is built; concurrent
    /// batches queue by `model_index` and apply post-batch under one
    /// write lock (queued by `model_index`, applied post-batch).
    pub app_state_patch: Option<AppStatePatch>,
    /// Declarative permission-rule deltas to fold into the running
    /// session config. Applied via the executor's
    /// `PermissionRuleHandle` at the same point as `app_state_patch`.
    pub permission_updates: Vec<PermissionUpdate>,
    // Future effects (pending cache invalidations, telemetry
    // side-channels, etc.) live here — they do NOT leak into the
    // history-facing outcome.
}

impl ToolSideEffects {
    /// Empty side-effect set — used when the runner has no patch to
    /// return (the common case for pre-execution EarlyOutcome plans).
    pub fn none() -> Self {
        Self {
            app_state_patch: None,
            permission_updates: Vec::new(),
        }
    }
}

impl UnstampedToolCallOutcome {
    /// Surface-time operation owned by the executor.
    ///
    /// Splits the outcome into:
    ///
    /// 1. A history-facing [`ToolCallOutcome`] (patch-free; safe to
    ///    hand to `on_outcome` the moment the future resolves).
    /// 2. [`ToolSideEffects`] the executor keeps until the right
    ///    application moment (serial: before the next tool's
    ///    `ToolUseContext`; concurrent-safe batch: under one write
    ///    lock at end-of-batch, iterated in `model_index` order).
    ///
    /// Visibility is `pub(crate)` so only the executor (same crate)
    /// can call it. `ToolCallRunner` in `coco-query` returns the
    /// unstamped body and never sees this method, which keeps
    /// "executor stamps completion_seq" a type-system guarantee
    /// rather than a convention.
    pub(crate) fn stamp_and_extract_effects(
        self,
        completion_seq: usize,
    ) -> (ToolCallOutcome, ToolSideEffects) {
        let Self {
            tool_use_id,
            tool_id,
            model_index,
            ordered_messages,
            message_path,
            error_kind,
            permission_denial,
            prevent_continuation,
            structured_output,
            effects,
        } = self;
        let outcome = ToolCallOutcome {
            tool_use_id,
            tool_id,
            model_index,
            ordered_messages,
            message_path,
            error_kind,
            permission_denial,
            prevent_continuation,
            structured_output,
            completion_seq,
        };
        (outcome, effects)
    }
}

/// The final, stamped outcome delivered to the history-append
/// callback.
///
/// Fields are **private**; the only constructor lives in
/// [`UnstampedToolCallOutcome::stamp_and_extract_effects`] (also
/// `pub(crate)`). External crates cannot fabricate or mutate a
/// `ToolCallOutcome`, which makes "executor-only stamping" a
/// compile-time invariant. Read access goes through the explicit
/// accessor methods below.
///
/// Patch-free by construction: any `AppStatePatch` that was in the
/// unstamped body now lives in [`ToolSideEffects`], held by the
/// executor until the correct apply moment.
pub struct ToolCallOutcome {
    tool_use_id: String,
    tool_id: ToolId,
    model_index: usize,
    ordered_messages: Vec<Message>,
    message_path: ToolMessagePath,
    error_kind: Option<ToolCallErrorKind>,
    permission_denial: Option<PermissionDenialInfo>,
    prevent_continuation: Option<String>,
    structured_output: Option<serde_json::Value>,
    completion_seq: usize,
}

impl ToolCallOutcome {
    pub fn tool_use_id(&self) -> &str {
        &self.tool_use_id
    }
    pub fn tool_id(&self) -> &ToolId {
        &self.tool_id
    }
    pub fn model_index(&self) -> usize {
        self.model_index
    }
    pub fn completion_seq(&self) -> usize {
        self.completion_seq
    }
    /// Pre-flattened messages in model index order. The engine appends these
    /// verbatim — it must not re-sort or re-resolve the tool.
    pub fn ordered_messages(&self) -> &[Message] {
        &self.ordered_messages
    }
    /// Structured view for telemetry / tests only; never re-flatten.
    pub fn message_path(&self) -> ToolMessagePath {
        self.message_path
    }
    pub fn error_kind(&self) -> Option<&ToolCallErrorKind> {
        self.error_kind.as_ref()
    }
    pub fn permission_denial(&self) -> Option<&PermissionDenialInfo> {
        self.permission_denial.as_ref()
    }
    pub fn prevent_continuation(&self) -> Option<&str> {
        self.prevent_continuation.as_deref()
    }
    /// SDK-facing structured output, propagated from the success-path
    /// runner. `None` for failure / early-return paths.
    pub fn structured_output(&self) -> Option<&serde_json::Value> {
        self.structured_output.as_ref()
    }
    /// Destructure into owned parts (history-append consumes
    /// `ordered_messages`). Does NOT expose any `AppStatePatch` —
    /// patches live in [`ToolSideEffects`], not here.
    pub fn into_parts(self) -> ToolCallOutcomeParts {
        let Self {
            tool_use_id,
            tool_id,
            model_index,
            ordered_messages,
            message_path,
            error_kind,
            permission_denial,
            prevent_continuation,
            structured_output,
            completion_seq,
        } = self;
        ToolCallOutcomeParts {
            tool_use_id,
            tool_id,
            model_index,
            ordered_messages,
            message_path,
            error_kind,
            permission_denial,
            prevent_continuation,
            structured_output,
            completion_seq,
        }
    }
}

/// Owned decomposition of [`ToolCallOutcome`] for consumers that need
/// to move `ordered_messages` into history.
pub struct ToolCallOutcomeParts {
    pub tool_use_id: String,
    pub tool_id: ToolId,
    pub model_index: usize,
    pub ordered_messages: Vec<Message>,
    pub message_path: ToolMessagePath,
    pub error_kind: Option<ToolCallErrorKind>,
    pub permission_denial: Option<PermissionDenialInfo>,
    pub prevent_continuation: Option<String>,
    pub structured_output: Option<serde_json::Value>,
    pub completion_seq: usize,
}

/// Which lifecycle path produced a given outcome.
///
/// Mirrors the `app/query::tool_message::ToolMessagePath` helper; kept
/// separate so the type is available across the scheduler boundary
/// without pulling `coco-query` types into `coco-tool`. The runner
/// resolves the `app/query` path and surfaces the matching variant
/// here for telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMessagePath {
    /// `tool.execute()` ran to completion. Post-hook is `PostToolUse`.
    Success,
    /// `tool.execute()` threw. Post-hook is `PostToolUseFailure`.
    Failure,
    /// Unknown tool / schema / pre-hook stop / permission denied.
    EarlyReturn,
}

/// Synthetic error classification for EarlyReturn and Failure paths.
///
/// Every model-visible error maps to exactly one variant so tests and
/// telemetry can assert the lifecycle stage that produced the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallErrorKind {
    UnknownTool,
    /// Schema validation failed — either the initial check in
    /// `prepare_batch` or the re-validation after a PreToolUse hook
    /// rewrote the input (Rust-side tightening, per I3).
    SchemaFailed,
    /// `tool.validate_input()` rejected the effective input.
    ValidationFailed,
    /// PreToolUse hook returned a hard block.
    HookBlocked,
    PermissionDenied,
    PermissionBridgeFailed,
    /// Exception raised by `tool.execute()`.
    ExecutionFailed,
    /// Cancellation observed **before** `tool.execute()` started
    /// (during prepare / validation / hook / permission stages).
    /// Does NOT fire PostToolUseFailure hooks.
    PreExecutionCancelled,
    /// Cancellation observed **after** `tool.execute()` started.
    /// DOES fire PostToolUseFailure hooks.
    ExecutionCancelled,
    /// `tokio::JoinError`. Only reachable once the spawned future is
    /// in flight, so treated as execution-stage.
    JoinFailed,
    /// Streaming fallback discarded the call before it completed.
    StreamingDiscarded,
}

impl ToolCallErrorKind {
    /// Whether this error path should fire PostToolUseFailure hooks.
    ///
    /// Per I3 step 12: execution-stage fail runs failure hooks;
    /// pre-execution abort does NOT. The enum itself encodes the
    /// lifecycle stage, so this match is exhaustive and no
    /// `execution_started: bool` side-channel is needed.
    pub fn runs_post_tool_use_failure(self) -> bool {
        match self {
            Self::ExecutionFailed | Self::ExecutionCancelled | Self::JoinFailed => true,
            Self::UnknownTool
            | Self::SchemaFailed
            | Self::ValidationFailed
            | Self::HookBlocked
            | Self::PermissionDenied
            | Self::PermissionBridgeFailed
            | Self::PreExecutionCancelled
            | Self::StreamingDiscarded => false,
        }
    }

    /// Which lifecycle path this error belongs to.
    ///
    /// `Failure` for execution-stage errors (so the runner picks the
    /// PostToolUseFailure template); `EarlyReturn` for pre-execution
    /// errors (so the flatten template drops post-hook).
    pub fn message_path(self) -> ToolMessagePath {
        if self.runs_post_tool_use_failure() {
            ToolMessagePath::Failure
        } else {
            ToolMessagePath::EarlyReturn
        }
    }
}

#[cfg(test)]
#[path = "call_plan.test.rs"]
mod tests;
