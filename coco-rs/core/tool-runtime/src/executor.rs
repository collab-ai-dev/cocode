//! Concurrent tool executor.
//!
//! Drives a validated plan list via [`ToolExecutor::execute_with`]
//! (batch) or [`StreamingHandle`](crate::executor_streaming) (streaming):
//! concurrency-safe tools run in parallel under a semaphore, unsafe tools
//! run serially, outcomes surface in completion order (I12), and
//! `app_state` patches apply in model-index order under one write lock.

use coco_config::EnvKey;
use coco_config::env;
use coco_types::ToolAbortReasonPayload;
use coco_types::ToolId;
use coco_types::ToolName;
use coco_types::TuiOnlyEvent;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;

use crate::call_plan::PreparedToolCall;
use crate::call_plan::RunOneRuntime;
use crate::call_plan::ToolCallErrorKind;
use crate::call_plan::ToolCallOutcome;
use crate::call_plan::ToolCallPlan;
use crate::call_plan::ToolSideEffects;
use crate::call_plan::UnstampedToolCallOutcome;
use crate::cancellation::ToolAbortController;
use crate::cancellation::ToolAbortSignal;
use crate::cancellation::TurnAbortController;
use crate::cancellation::TurnAbortSignal;
use crate::traits::DynTool;
use crate::traits::InterruptBehavior;
use crate::validated_input::ValidatedInput;

/// Default maximum concurrent tool executions.
const DEFAULT_MAX_CONCURRENCY: usize = 10;

/// A pending tool call waiting for execution.
///
/// `input` is a [`ValidatedInput`] by construction: whoever builds a pending
/// call has already run freeform coercion + schema validation, so a raw
/// freeform string (apply_patch's `*** Begin Patch …` envelope) can never
/// reach `serde_json::from_value::<T::Input>` at execute time.
pub struct PendingToolCall {
    pub tool_use_id: String,
    pub tool: Arc<dyn DynTool>,
    pub input: ValidatedInput,
    pub is_concurrency_safe: bool,
}

impl std::fmt::Debug for PendingToolCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingToolCall")
            .field("tool_use_id", &self.tool_use_id)
            .field("tool_name", &self.tool.name())
            .finish()
    }
}

/// Concurrent tool executor — schedules already-prepared plans.
///
/// `execute_with` (batch) and `StreamingHandle` (streaming) both drive
/// this struct: concurrency-safe tools run in parallel under
/// `max_concurrency`, unsafe tools run serially, and a failing shell
/// tool aborts its concurrent siblings via `sibling_abort`.
pub struct ToolExecutor {
    max_concurrency: usize,
    /// Current turn abort signal for scheduler-based execution.
    turn_abort: TurnAbortSignal,
    /// Structured sibling-abort controller.
    sibling_abort: ToolAbortController,
    /// Shared app_state write handle — the executor owns the **only
    /// write-capable reference** visible from the tool pipeline.
    /// Tools see `ctx.app_state` as an `AppStateReadHandle` (no
    /// write method); they return mutations via
    /// `ToolResult::app_state_patch` which we apply here, under a
    /// single write lock per batch.
    app_state: Option<Arc<RwLock<coco_types::ToolAppState>>>,
    /// Optional permission-rule mutation handle. Applied at the same
    /// point as `app_state_patch` so rules emitted by a tool
    /// (typically `SkillTool` forwarding skill frontmatter
    /// `allowed-tools`) are visible to the next tool / turn. `None`
    /// resolves to a silent drop (test / standalone-executor paths).
    permission_rule_handle: Option<crate::PermissionRuleHandleRef>,
    /// Optional protocol-event sink used to broadcast
    /// `TaskPanelChanged` after every applied `app_state_patch`. Keeps
    /// the TUI in sync with V2 plan-item and V1 todo snapshots.
    event_tx: Option<mpsc::Sender<coco_types::CoreEvent>>,
}

/// Resolve the tool concurrency cap from the raw `COCO_MAX_TOOL_USE_CONCURRENCY`
/// value. Any falsy result — including `0` — falls back to the default.
/// A `0` here would build `Semaphore::new(0)` and deadlock every
/// concurrent-safe tool, so we filter non-positive values out.
fn resolve_max_concurrency(raw: Option<String>) -> usize {
    raw.and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_CONCURRENCY)
}

impl ToolExecutor {
    pub fn new() -> Self {
        let max_concurrency =
            resolve_max_concurrency(env::env_opt(EnvKey::CocoMaxToolUseConcurrency));
        Self {
            max_concurrency,
            turn_abort: TurnAbortController::new().signal(),
            sibling_abort: ToolAbortController::new(),
            app_state: None,
            permission_rule_handle: None,
            event_tx: None,
        }
    }

    /// Attach the shared app_state write handle. Must match the Arc
    /// wrapped inside `ToolUseContext.app_state` (a read-only view
    /// of the same store). Without this, patches returned by tools
    /// are silently dropped (the executor has nowhere to apply them).
    pub fn with_app_state(mut self, arc: Arc<RwLock<coco_types::ToolAppState>>) -> Self {
        self.app_state = Some(arc);
        self
    }

    /// Attach a permission-rule mutation handle. Tools that return
    /// `ToolResult::permission_updates` (today: `SkillTool` forwarding
    /// a skill's `allowed-tools` frontmatter) push deltas through this
    /// handle, for `alwaysAllowRules` updates. Without this, updates are silently dropped
    /// with a `tracing::debug!` (standalone executor / test paths).
    pub fn with_permission_rule_handle(mut self, handle: crate::PermissionRuleHandleRef) -> Self {
        self.permission_rule_handle = Some(handle);
        self
    }

    /// Install a protocol-event sink so the executor can emit
    /// `TaskPanelChanged` after applying task-related `app_state_patch`
    /// closures. Optional; omission drops the notifications silently
    /// (tests + SDK-only paths don't need UI refreshes).
    pub fn with_event_sink(mut self, tx: mpsc::Sender<coco_types::CoreEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    pub fn with_turn_abort(mut self, signal: TurnAbortSignal) -> Self {
        self.turn_abort = signal;
        self
    }

    // -- Scheduler API (plans + callback-driven surfacing) --
    //
    // `execute_with` is the batch scheduler: the runner hands in
    // pre-validated `ToolCallPlan` values and a `run_one` callback, and
    // the executor surfaces each outcome through `on_outcome` the moment
    // it is ready. No pre-allocated result-slot vector — history grows in
    // completion order for concurrent-safe batches, execution order for
    // serial unsafe tools, and partition order for `EarlyOutcome`
    // barriers. The streaming counterpart is `StreamingHandle`
    // (`executor_streaming.rs`); both share `run_concurrent_batch` /
    // `apply_side_effects` / `make_runtime`.

    /// Drive a plan list and surface each outcome through `on_outcome`
    /// as soon as it is available.
    ///
    /// Ordering contract (I12):
    ///
    /// - `ToolCallPlan::EarlyOutcome` acts as a single-tool barrier.
    ///   It splits the surrounding `Runnable` plans into separate
    ///   concurrent-safe batches (schema-invalid calls have
    ///   `isConcurrencySafe = false`).
    /// - Within a concurrent-safe batch, runnable plans dispatch
    ///   through a `FuturesUnordered` so a slow earlier tool does not
    ///   block a faster later tool. The executor stamps
    ///   `completion_seq` at surface time and calls `on_outcome`
    ///   immediately with the patch-free `ToolCallOutcome`.
    /// - Within a concurrent-safe batch, queued `app_state_patch`es
    ///   apply post-batch in **model_index** order under one write
    ///   lock.
    /// - Serial unsafe plans apply their patch before building the
    ///   next tool's context.
    /// - `EarlyOutcome` plans stamp when the partitioner reaches that
    ///   plan's block — not globally before all Runnables — so the
    ///   resulting completion sequence interleaves correctly with
    ///   surrounding batches.
    ///
    /// This does **not** emit `ToolUseStarted` / `ToolUseCompleted` —
    /// the engine owns those events at the runner boundary.
    pub async fn execute_with<F, Fut, H>(
        &self,
        plans: Vec<ToolCallPlan>,
        run_one: F,
        mut on_outcome: H,
    ) where
        F: Fn(PreparedToolCall, RunOneRuntime) -> Fut + Sync,
        Fut: std::future::Future<Output = UnstampedToolCallOutcome> + Send,
        H: FnMut(ToolCallOutcome),
    {
        let mut completion_seq: usize = 0;

        // Partition plans into blocks. Each Runnable sequence of
        // concurrency-safe tools becomes one batch; an EarlyOutcome or
        // unsafe Runnable breaks the batch and forms its own block.
        let blocks = partition_plans(plans);

        for block in blocks {
            match block {
                PlanBlock::EarlyOutcome(unstamped) => {
                    // Pre-execution failure: no scheduling, no patch
                    // — but still emit one Completed outcome so the
                    // per-call invariant (one Queued → one Completed)
                    // holds.
                    let (outcome, _effects) = unstamped.stamp_and_extract_effects(completion_seq);
                    completion_seq += 1;
                    on_outcome(outcome);
                }
                PlanBlock::SerialUnsafe(prepared) => {
                    self.emit_interruptibility(interruptible_set(std::slice::from_ref(&prepared)))
                        .await;
                    let runtime = self.make_runtime(prepared.model_index);
                    let unstamped = run_one(prepared, runtime).await;
                    let (outcome, effects) = unstamped.stamp_and_extract_effects(completion_seq);
                    completion_seq += 1;
                    // Apply patch BEFORE the next tool's context build.
                    // This is the serial-tool equivalent of
                    // "update.newContext()".
                    self.apply_side_effects(effects).await;
                    on_outcome(outcome);
                    self.emit_interruptibility(false).await;
                }
                PlanBlock::ConcurrentSafe(prepared_calls) => {
                    self.emit_interruptibility(interruptible_set(&prepared_calls))
                        .await;
                    self.run_concurrent_batch(
                        prepared_calls,
                        &run_one,
                        &mut on_outcome,
                        &mut completion_seq,
                    )
                    .await;
                    self.emit_interruptibility(false).await;
                }
            }
        }
    }

    /// Build a fresh per-tool runtime for one `run_one` invocation.
    pub(crate) fn make_runtime(&self, model_index: usize) -> RunOneRuntime {
        let self_abort = ToolAbortController::new();
        RunOneRuntime {
            abort: ToolAbortSignal::new(
                self.turn_abort.clone(),
                self_abort.signal(),
                Some(self.sibling_abort.signal()),
            ),
            model_index,
        }
    }

    /// Fire the sibling-abort controller when a shell tool (Bash/PowerShell)
    /// fails, so concurrently-running safe siblings self-cancel (they listen on
    /// the same controller via [`make_runtime`](Self::make_runtime)). Shared by
    /// the non-streaming `run_concurrent_batch` and the streaming
    /// `start_safe_now` completion path so both surfaces behave identically.
    /// tool-runtime#8.
    pub(crate) fn abort_siblings_if_shell_error(
        &self,
        error_kind: Option<&ToolCallErrorKind>,
        tool_id: &ToolId,
    ) {
        if error_kind.is_some() && is_shell_tool_id(tool_id) {
            self.sibling_abort
                .abort(ToolAbortReasonPayload::SiblingError {
                    failed_tool: tool_id.to_string(),
                });
        }
    }

    /// Run one concurrent-safe batch end-to-end.
    ///
    /// Surfaces each outcome through `on_outcome` the moment
    /// `run_one` resolves (completion-order history). Queues
    /// `app_state_patch`es keyed by `model_index` and applies them in
    /// model-index order post-batch.
    pub(crate) async fn run_concurrent_batch<F, Fut, H>(
        &self,
        prepared_calls: Vec<PreparedToolCall>,
        run_one: &F,
        on_outcome: &mut H,
        completion_seq: &mut usize,
    ) where
        F: Fn(PreparedToolCall, RunOneRuntime) -> Fut + Sync,
        Fut: std::future::Future<Output = UnstampedToolCallOutcome> + Send,
        H: FnMut(ToolCallOutcome),
    {
        let semaphore = Arc::new(Semaphore::new(self.max_concurrency));
        let mut pending: FuturesUnordered<_> = FuturesUnordered::new();

        for prepared in prepared_calls {
            let runtime = self.make_runtime(prepared.model_index);
            let sem = semaphore.clone();
            let fut = run_one(prepared, runtime);
            pending.push(async move {
                // Semaphore acquisition bounds the concurrent tool
                // count but does not block the driver future from
                // progressing — a permit holder returns the permit
                // when it drops.
                let _permit = sem.acquire().await.ok();
                fut.await
            });
        }

        let mut queued_effects: Vec<(usize, ToolSideEffects)> = Vec::new();

        while let Some(unstamped) = pending.next().await {
            let model_index = unstamped.model_index;
            self.abort_siblings_if_shell_error(unstamped.error_kind.as_ref(), &unstamped.tool_id);
            let (outcome, effects) = unstamped.stamp_and_extract_effects(*completion_seq);
            *completion_seq += 1;
            queued_effects.push((model_index, effects));
            on_outcome(outcome);
        }

        // Apply queued patches in model_index order under one write
        // lock — concurrent-safe tools surface in completion order but
        // their state mutations must apply deterministically (I12).
        queued_effects.sort_by_key(|(idx, _)| *idx);
        let (patches, update_lists): (Vec<_>, Vec<_>) = queued_effects
            .into_iter()
            .map(|(_, e)| (e.app_state_patch, e.permission_updates))
            .unzip();
        let combined = ToolSideEffects {
            app_state_patch: coalesce_patches(patches.into_iter().flatten()),
            permission_updates: update_lists.into_iter().flatten().collect(),
        };
        self.apply_side_effects(combined).await;
    }

    /// Apply a `ToolSideEffects` under one write lock, emitting a
    /// `TaskPanelChanged` snapshot to the event sink when a patch
    /// actually ran. Matches the existing legacy-path invariants:
    /// patch `FnOnce` runs exactly once, event is best-effort
    /// delivery (dropped if no sink is configured).
    pub(crate) async fn apply_side_effects(&self, effects: ToolSideEffects) {
        let ToolSideEffects {
            app_state_patch,
            permission_updates,
        } = effects;

        // Permission-rule updates apply via the dedicated handle and
        // are independent of the app_state patch. Run them first so a
        // missing `app_state` (i.e. early-return below) doesn't drop
        // the permission delta.
        if !permission_updates.is_empty()
            && let Some(handle) = self.permission_rule_handle.as_ref()
        {
            handle.apply_updates(permission_updates).await;
        }

        let Some(patch) = app_state_patch else {
            return;
        };
        let Some(state) = self.app_state.as_ref() else {
            // No shared state → drop the patch; the context modifier
            // is never invoked when there's no context to modify.
            return;
        };
        let snapshot = {
            let mut guard = state.write().await;
            patch(&mut guard);
            coco_types::TaskPanelChangedParams {
                plan_tasks: guard.plan_tasks.clone(),
                todos_by_agent: guard.todos_by_agent.clone(),
                expanded_view: guard.expanded_view,
                verification_nudge_pending: guard.verification_nudge_pending,
            }
        };
        if let Some(tx) = self.event_tx.as_ref() {
            let _ = tx
                .send(coco_types::CoreEvent::Protocol(
                    coco_types::ServerNotification::TaskPanelChanged(snapshot),
                ))
                .await;
        }
    }

    async fn emit_interruptibility(&self, interruptible: bool) {
        let Some(tx) = self.event_tx.as_ref() else {
            return;
        };
        let _ = tx
            .send(coco_types::CoreEvent::Tui(
                TuiOnlyEvent::ToolInterruptibilityChanged { interruptible },
            ))
            .await;
    }
}

fn interruptible_set(prepared_calls: &[PreparedToolCall]) -> bool {
    !prepared_calls.is_empty()
        && prepared_calls
            .iter()
            .all(|prepared| prepared.tool.interrupt_behavior() == InterruptBehavior::Cancel)
}

pub(crate) fn is_shell_tool_id(tool_id: &ToolId) -> bool {
    matches!(
        tool_id,
        ToolId::Builtin(ToolName::Bash) | ToolId::Builtin(ToolName::PowerShell)
    )
}

/// One block in the executor's plan-partition output.
///
/// `ConcurrentSafe` holds one-or-more `Runnable` plans that can run
/// in parallel; `SerialUnsafe` holds a single non-concurrency-safe
/// `Runnable`; `EarlyOutcome` passes a pre-built outcome straight to
/// the stamp path.
enum PlanBlock {
    ConcurrentSafe(Vec<PreparedToolCall>),
    SerialUnsafe(PreparedToolCall),
    EarlyOutcome(UnstampedToolCallOutcome),
}

/// Partition a flat plan list into batches.
///
/// Rules:
///
/// - `EarlyOutcome` is never concurrency-safe — it ends the preceding
///   safe batch and forms its own block.
/// - `Runnable` whose `is_concurrency_safe(input)` returns `false`
///   forms its own `SerialUnsafe` block.
/// - `Runnable` whose `is_concurrency_safe(input)` returns `true`
///   accumulates into a `ConcurrentSafe` batch, flushed whenever an
///   `EarlyOutcome` or unsafe Runnable interrupts.
/// - If `is_concurrency_safe` panics (defensive), treat as unsafe.
fn partition_plans(plans: Vec<ToolCallPlan>) -> Vec<PlanBlock> {
    let mut blocks: Vec<PlanBlock> = Vec::new();
    let mut safe_batch: Vec<PreparedToolCall> = Vec::new();
    let flush_safe = |safe_batch: &mut Vec<PreparedToolCall>, blocks: &mut Vec<PlanBlock>| {
        if !safe_batch.is_empty() {
            blocks.push(PlanBlock::ConcurrentSafe(std::mem::take(safe_batch)));
        }
    };
    for plan in plans {
        match plan {
            ToolCallPlan::Runnable(prepared) => {
                let is_safe = prepared.is_concurrency_safe;
                if is_safe {
                    safe_batch.push(prepared);
                } else {
                    flush_safe(&mut safe_batch, &mut blocks);
                    blocks.push(PlanBlock::SerialUnsafe(prepared));
                }
            }
            ToolCallPlan::EarlyOutcome(unstamped) => {
                flush_safe(&mut safe_batch, &mut blocks);
                blocks.push(PlanBlock::EarlyOutcome(unstamped));
            }
        }
    }
    flush_safe(&mut safe_batch, &mut blocks);
    blocks
}

/// Coalesce several `FnOnce` patches into a single one that invokes
/// each in turn. Used so the end-of-batch apply is still exactly one
/// `write().await` + one TaskPanelChanged snapshot, regardless of how
/// many tools in the batch emitted patches.
pub(crate) fn coalesce_patches(
    patches: impl IntoIterator<Item = coco_types::AppStatePatch>,
) -> Option<coco_types::AppStatePatch> {
    let patches: Vec<coco_types::AppStatePatch> = patches.into_iter().collect();
    if patches.is_empty() {
        return None;
    }
    Some(Box::new(move |state| {
        for patch in patches {
            patch(state);
        }
    }))
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "executor.test.rs"]
mod tests;
