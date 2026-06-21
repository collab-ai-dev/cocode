//! Tests for [`StreamingHandle`] — Phase 9.
//!
//! Locks in:
//! - safe plans start mid-stream (verified via atomic counter)
//! - unsafe plans hold for commit_flush
//! - once an unsafe plan is fed, later safe plans also hold
//! - discard produces `StreamingDiscarded` for held plans; no
//!   panic / leak for in-flight ones (Drop cancels the futures)
//! - commit_flush surfaces outcomes + applies patches in model order

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use coco_messages::ToolResult;
use coco_types::ToolId;
use serde_json::Value;
use serde_json::json;

use crate::call_plan::PreparedToolCall;
use crate::call_plan::ToolCallPlan;
use crate::call_plan::ToolSideEffects;
use crate::call_plan::UnstampedToolCallOutcome;
use crate::executor::ToolExecutor;
use crate::traits::DescriptionOptions;

/// A tool whose `is_concurrency_safe` flag is configurable, and whose
/// `execute` increments a per-tool "started" counter so tests can
/// assert when the tool ran.
struct ConfigurableTool {
    name: String,
    safe: bool,
    started_counter: Arc<AtomicI32>,
    sleep_ms: u64,
}

#[async_trait::async_trait]
impl crate::traits::Tool for ConfigurableTool {
    fn runtime_validation_schema(&self) -> &crate::schema::ToolInputSchema {
        crate::schema::test_runtime_schema()
    } // Migration scaffold: assoc types pinned to `Value`.
    type Input = serde_json::Value;
    type Output = serde_json::Value;

    fn id(&self) -> ToolId {
        ToolId::Custom(self.name.clone())
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "configurable".into()
    }
    async fn prompt(&self, _options: &crate::traits::PromptOptions) -> String {
        "test tool".into()
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        self.safe
    }
    async fn execute(
        &self,
        _input: Value,
        _ctx: &crate::context::ToolUseContext,
    ) -> Result<ToolResult<Value>, crate::error::ToolError> {
        self.started_counter.fetch_add(1, Ordering::SeqCst);
        if self.sleep_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
        }
        Ok(ToolResult {
            data: json!({}),
            new_messages: Vec::new(),
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

fn prepared(
    name: &str,
    model_index: usize,
    safe: bool,
    started: Arc<AtomicI32>,
    sleep_ms: u64,
) -> PreparedToolCall {
    let tool: Arc<dyn crate::traits::DynTool> = Arc::new(ConfigurableTool {
        name: name.into(),
        safe,
        started_counter: started,
        sleep_ms,
    });
    PreparedToolCall {
        tool_use_id: name.into(),
        tool_id: ToolId::Custom(name.into()),
        parsed_input: crate::ValidatedInput::validate(tool.as_ref(), json!({}))
            .expect("test input must validate"),
        is_concurrency_safe: safe,
        tool,
        model_index,
        permission_resolution_detail: None,
        approval_feedback: None,
    }
}

/// `run_one` that executes the tool and wraps the result in an
/// `UnstampedToolCallOutcome` shaped like the runner would.
async fn stub_run_one(
    prepared: PreparedToolCall,
    _runtime: crate::call_plan::RunOneRuntime,
) -> UnstampedToolCallOutcome {
    let ctx = crate::context::ToolUseContext::test_default();
    let _ = prepared
        .tool
        .execute(prepared.parsed_input.into_value(), &ctx)
        .await;
    UnstampedToolCallOutcome {
        tool_use_id: prepared.tool_use_id,
        tool_id: prepared.tool_id,
        model_index: prepared.model_index,
        ordered_messages: Vec::new(),
        message_path: crate::call_plan::ToolMessagePath::Success,
        error_kind: None,
        permission_denial: None,
        prevent_continuation: None,
        structured_output: None,
        effects: ToolSideEffects::none(),
    }
}

// ── feed_plan: safe starts immediately ───────────────────────────

#[tokio::test]
async fn test_safe_plan_starts_mid_stream() {
    let executor = Arc::new(ToolExecutor::new());
    let started = Arc::new(AtomicI32::new(0));
    let mut handle = executor.streaming_handle(stub_run_one);

    // Feed one safe plan with a 50ms sleep — it should start NOW,
    // not wait for commit_flush.
    handle.feed_plan(ToolCallPlan::Runnable(prepared(
        "safe_a",
        0,
        /*safe*/ true,
        started.clone(),
        50,
    )));

    // Give the scheduler a tick to pick up the plan.
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "safe plan must start as soon as fed; counter was not incremented"
    );

    // Drain cleanly.
    let mut collected: Vec<String> = Vec::new();
    handle
        .commit_flush(0, |o| collected.push(o.tool_use_id().to_string()))
        .await;
    assert_eq!(collected, vec!["safe_a"]);
}

// ── feed_plan: unsafe holds; subsequent safes also hold ──────────

#[tokio::test]
async fn test_unsafe_plan_holds_until_commit_flush() {
    let executor = Arc::new(ToolExecutor::new());
    let started = Arc::new(AtomicI32::new(0));
    let mut handle = executor.streaming_handle(stub_run_one);

    handle.feed_plan(ToolCallPlan::Runnable(prepared(
        "unsafe_a",
        0,
        /*safe*/ false,
        started.clone(),
        0,
    )));

    // Wait a tick — unsafe must NOT have started.
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "unsafe plan must not start before commit_flush"
    );

    // Flush commits; now it runs.
    let mut collected: Vec<String> = Vec::new();
    handle
        .commit_flush(0, |o| collected.push(o.tool_use_id().to_string()))
        .await;
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert_eq!(collected, vec!["unsafe_a"]);
}

#[tokio::test]
async fn test_unsafe_feed_poisons_subsequent_safe_starts() {
    // Rule: once any unsafe plan is fed, every subsequent safe feed
    // also waits. This preserves "no safe/unsafe interleave
    // mid-stream" without requiring tool-level interlock.
    let executor = Arc::new(ToolExecutor::new());
    let started = Arc::new(AtomicI32::new(0));
    let mut handle = executor.streaming_handle(stub_run_one);

    // Safe #1 starts immediately.
    handle.feed_plan(ToolCallPlan::Runnable(prepared(
        "safe_before",
        0,
        /*safe*/ true,
        started.clone(),
        0,
    )));
    // Unsafe fed now.
    handle.feed_plan(ToolCallPlan::Runnable(prepared(
        "unsafe",
        1,
        /*safe*/ false,
        started.clone(),
        0,
    )));
    // Safe #2 fed after unsafe → MUST hold, not start immediately.
    handle.feed_plan(ToolCallPlan::Runnable(prepared(
        "safe_after",
        2,
        /*safe*/ true,
        started.clone(),
        0,
    )));

    // Give scheduler a tick. Only safe_before should have started.
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "only safe_before should have started; safe_after must be queued"
    );

    let mut collected: Vec<String> = Vec::new();
    handle
        .commit_flush(0, |o| collected.push(o.tool_use_id().to_string()))
        .await;
    assert_eq!(started.load(Ordering::SeqCst), 3);
    // Order: safe_before completes in flush drain; then pending_serial
    // runs in feed order = unsafe, safe_after.
    assert_eq!(collected, vec!["safe_before", "unsafe", "safe_after"]);
}

// ── feed_plan: EarlyOutcome passed through ───────────────────────

#[tokio::test]
async fn test_early_outcome_surfaces_first_on_flush() {
    let executor = Arc::new(ToolExecutor::new());
    let started = Arc::new(AtomicI32::new(0));
    let mut handle = executor.streaming_handle(stub_run_one);

    // EarlyOutcome: pre-resolved synthetic error.
    handle.feed_plan(ToolCallPlan::EarlyOutcome(UnstampedToolCallOutcome {
        tool_use_id: "unknown_tool".into(),
        tool_id: ToolId::Custom("unknown_tool".into()),
        model_index: 0,
        ordered_messages: Vec::new(),
        message_path: crate::call_plan::ToolMessagePath::EarlyReturn,
        error_kind: Some(crate::call_plan::ToolCallErrorKind::UnknownTool),
        permission_denial: None,
        prevent_continuation: None,
        structured_output: None,
        effects: ToolSideEffects::none(),
    }));
    // One safe plan after.
    handle.feed_plan(ToolCallPlan::Runnable(prepared(
        "safe",
        1,
        true,
        started.clone(),
        0,
    )));

    let mut ids: Vec<String> = Vec::new();
    handle
        .commit_flush(0, |o| ids.push(o.tool_use_id().to_string()))
        .await;
    // EarlyOutcome surfaces first, then safe.
    assert_eq!(ids, vec!["unknown_tool", "safe"]);
}

// ── discard: produces StreamingDiscarded for pending plans ────────

#[tokio::test]
async fn test_discard_converts_pending_to_streaming_discarded() {
    let executor = Arc::new(ToolExecutor::new());
    let started = Arc::new(AtomicI32::new(0));
    let mut handle = executor.streaming_handle(stub_run_one);

    handle.feed_plan(ToolCallPlan::Runnable(prepared(
        "unsafe_pending",
        0,
        /*safe*/ false,
        started.clone(),
        0,
    )));

    let out = handle.discard().await;
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0].error_kind,
        Some(crate::call_plan::ToolCallErrorKind::StreamingDiscarded)
    );
    // The plan never started because discard happened before
    // commit_flush.
    assert_eq!(started.load(Ordering::SeqCst), 0);
}

// ── terminal_drain: budget-stop semantics ────────────────────────

#[tokio::test]
async fn test_terminal_drain_commits_ready_safe_and_skips_pending_serial() {
    let executor = Arc::new(ToolExecutor::new());
    let started = Arc::new(AtomicI32::new(0));
    let mut handle = executor.streaming_handle(stub_run_one);

    handle.feed_plan(ToolCallPlan::Runnable(prepared(
        "safe_done",
        0,
        /*safe*/ true,
        started.clone(),
        0,
    )));
    tokio::time::sleep(Duration::from_millis(10)).await;
    handle.feed_plan(ToolCallPlan::Runnable(prepared(
        "unsafe_pending",
        1,
        /*safe*/ false,
        started.clone(),
        0,
    )));

    let mut ids: Vec<String> = Vec::new();
    handle
        .terminal_drain(0, |o| ids.push(o.tool_use_id().to_string()))
        .await;

    assert_eq!(ids, vec!["safe_done"]);
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "queued unsafe plan must not run during terminal drain"
    );
}

#[tokio::test]
async fn test_terminal_drain_aborts_unfinished_safe_tool() {
    let executor = Arc::new(ToolExecutor::new());
    let started = Arc::new(AtomicI32::new(0));
    let mut handle = executor.streaming_handle(stub_run_one);

    handle.feed_plan(ToolCallPlan::Runnable(prepared(
        "safe_slow",
        0,
        /*safe*/ true,
        started.clone(),
        50,
    )));
    tokio::time::sleep(Duration::from_millis(10)).await;

    let mut ids: Vec<String> = Vec::new();
    handle
        .terminal_drain(0, |o| ids.push(o.tool_use_id().to_string()))
        .await;

    assert!(ids.is_empty());
    assert_eq!(started.load(Ordering::SeqCst), 1);
}

// ── commit_flush: completion_seq monotonic across the stream ──────

#[tokio::test]
async fn test_commit_flush_stamps_monotonic_completion_seq() {
    let executor = Arc::new(ToolExecutor::new());
    let started = Arc::new(AtomicI32::new(0));
    let mut handle = executor.streaming_handle(stub_run_one);

    for i in 0..3 {
        handle.feed_plan(ToolCallPlan::Runnable(prepared(
            &format!("safe_{i}"),
            i,
            true,
            started.clone(),
            0,
        )));
    }

    let mut seqs: Vec<usize> = Vec::new();
    handle
        .commit_flush(/*seq_start*/ 10, |o| seqs.push(o.completion_seq()))
        .await;
    assert_eq!(seqs.len(), 3);
    // Monotonic starting at 10 — exact identity of each depends on
    // completion order (non-deterministic among safe plans), but the
    // set of seqs must be {10, 11, 12}.
    seqs.sort();
    assert_eq!(seqs, vec![10, 11, 12]);
}

// ── tool-runtime#8: streaming-path sibling-abort on shell-tool failure ──
//
// Mirrors the non-streaming `test_execute_with_bash_failure_aborts_concurrent_
// sibling_runtime` but drives through `feed_plan` + `commit_flush`. A
// concurrency-safe Bash plan fails (error_kind set); the concurrently-running
// sibling must observe a `SiblingError` abort and self-cancel. Before the fix
// the streaming path had no sibling-abort trigger and the sibling would run to
// completion (the test would hang waiting on its `abort.cancelled()`).
#[tokio::test]
async fn test_streaming_shell_failure_aborts_concurrent_sibling() {
    use std::sync::Mutex;

    let executor = Arc::new(ToolExecutor::new());
    let observed = Arc::new(Mutex::new(None));
    let observed_for_run = observed.clone();

    let run_one = move |prepared: PreparedToolCall, runtime: crate::call_plan::RunOneRuntime| {
        let observed = observed_for_run.clone();
        async move {
            let mut outcome = UnstampedToolCallOutcome {
                tool_use_id: prepared.tool_use_id.clone(),
                tool_id: prepared.tool_id.clone(),
                model_index: prepared.model_index,
                ordered_messages: Vec::new(),
                message_path: crate::call_plan::ToolMessagePath::Failure,
                error_kind: None,
                permission_denial: None,
                prevent_continuation: None,
                structured_output: None,
                effects: ToolSideEffects::none(),
            };
            if prepared.tool_use_id == "bash-call" {
                // Fail after a tick so the sibling is already awaiting.
                tokio::time::sleep(Duration::from_millis(10)).await;
                outcome.error_kind = Some(crate::call_plan::ToolCallErrorKind::ExecutionFailed);
            } else {
                // Sibling: block until the sibling-abort fires, then record why.
                runtime.abort.cancelled().await;
                *observed.lock().unwrap() = runtime.abort.reason();
                outcome.error_kind = Some(crate::call_plan::ToolCallErrorKind::ExecutionCancelled);
            }
            outcome
        }
    };

    let mut handle = executor.streaming_handle(run_one);

    // Bash plan — concurrency-safe (read-only), tool_id = Bash so the shell
    // predicate matches; runs concurrently in the inflight JoinSet.
    let bash_tool: Arc<dyn crate::traits::DynTool> = Arc::new(ConfigurableTool {
        name: "bash".into(),
        safe: true,
        started_counter: Arc::new(AtomicI32::new(0)),
        sleep_ms: 0,
    });
    let bash = PreparedToolCall {
        tool_use_id: "bash-call".into(),
        tool_id: ToolId::Builtin(coco_types::ToolName::Bash),
        parsed_input: crate::ValidatedInput::validate(bash_tool.as_ref(), json!({}))
            .expect("test input must validate"),
        is_concurrency_safe: true,
        tool: bash_tool,
        model_index: 0,
        permission_resolution_detail: None,
        approval_feedback: None,
    };
    handle.feed_plan(ToolCallPlan::Runnable(bash));
    handle.feed_plan(ToolCallPlan::Runnable(prepared(
        "sibling",
        1,
        /*safe*/ true,
        Arc::new(AtomicI32::new(0)),
        0,
    )));

    let mut collected: Vec<String> = Vec::new();
    handle
        .commit_flush(0, |o| collected.push(o.tool_use_id().to_string()))
        .await;

    assert_eq!(collected.len(), 2);
    assert!(
        matches!(
            observed.lock().unwrap().as_ref(),
            Some(coco_types::ToolAbortReasonPayload::SiblingError { failed_tool })
                if failed_tool == coco_types::ToolName::Bash.as_str()
        ),
        "sibling must observe SiblingError(Bash); got {:?}",
        observed.lock().unwrap()
    );
}
