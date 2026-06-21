use coco_messages::ToolResult;
use coco_types::ToolId;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;

use super::*;
use crate::traits::DescriptionOptions;

/// A test tool that is concurrency-safe.
struct SafeTool {
    name: String,
}

#[async_trait::async_trait]
impl crate::traits::Tool for SafeTool {
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
    fn description(&self, _: &Value, _options: &DescriptionOptions) -> String {
        "safe".into()
    }
    async fn prompt(&self, _options: &crate::traits::PromptOptions) -> String {
        "test tool".into()
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    async fn execute(
        &self,
        input: Value,
        _ctx: &crate::context::ToolUseContext,
    ) -> Result<ToolResult<Value>, crate::error::ToolError> {
        Ok(ToolResult {
            data: input,
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

/// A test tool that is NOT concurrency-safe.
struct UnsafeTool {
    name: String,
}

#[async_trait::async_trait]
impl crate::traits::Tool for UnsafeTool {
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
    fn description(&self, _: &Value, _options: &DescriptionOptions) -> String {
        "unsafe".into()
    }
    async fn prompt(&self, _options: &crate::traits::PromptOptions) -> String {
        "test tool".into()
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }
    async fn execute(
        &self,
        input: Value,
        _ctx: &crate::context::ToolUseContext,
    ) -> Result<ToolResult<Value>, crate::error::ToolError> {
        Ok(ToolResult {
            data: input,
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

/// A slow tool that tracks concurrent execution via an atomic counter.
struct SlowSafeTool {
    name: String,
    concurrent_count: Arc<AtomicI32>,
    max_concurrent: Arc<AtomicI32>,
    sleep_ms: u64,
}

#[async_trait::async_trait]
impl crate::traits::Tool for SlowSafeTool {
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
    fn description(&self, _: &Value, _options: &DescriptionOptions) -> String {
        "slow safe".into()
    }
    async fn prompt(&self, _options: &crate::traits::PromptOptions) -> String {
        "test tool".into()
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    async fn execute(
        &self,
        _input: Value,
        _ctx: &crate::context::ToolUseContext,
    ) -> Result<ToolResult<Value>, crate::error::ToolError> {
        let prev = self.concurrent_count.fetch_add(1, Ordering::SeqCst);
        // Track peak concurrency
        let current = prev + 1;
        self.max_concurrent.fetch_max(current, Ordering::SeqCst);

        tokio::time::sleep(tokio::time::Duration::from_millis(self.sleep_ms)).await;

        self.concurrent_count.fetch_sub(1, Ordering::SeqCst);
        Ok(ToolResult {
            data: json!({"tool": self.name}),
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

// ══════════════════════════════════════════════════════════════════
// Phase 4d-α: execute_with scheduler tests
// ══════════════════════════════════════════════════════════════════
//
// Locks in the I12 ordering contract for the new callback-driven
// scheduler: concurrent batches surface in completion order, serial
// tools apply patches pre-next-tool, EarlyOutcome acts as a barrier,
// and concurrent-batch patches apply in model_index order
// post-batch.

use crate::call_plan::PreparedToolCall;
use crate::call_plan::ToolCallOutcome;
use crate::call_plan::ToolCallPlan;
use crate::call_plan::ToolMessagePath;
use crate::call_plan::ToolSideEffects;
use crate::call_plan::UnstampedToolCallOutcome;
use std::sync::Mutex;
use std::time::Duration;

fn prepared_from(
    tool: Arc<dyn crate::traits::DynTool>,
    tool_use_id: &str,
    model_index: usize,
) -> PreparedToolCall {
    PreparedToolCall {
        tool_use_id: tool_use_id.into(),
        tool_id: tool.id(),
        parsed_input: crate::ValidatedInput::validate(tool.as_ref(), json!({}))
            .expect("test input must validate"),
        is_concurrency_safe: tool.is_concurrency_safe(&json!({})),
        tool,
        model_index,
        permission_resolution_detail: None,
        approval_feedback: None,
    }
}

fn empty_unstamped(tool_use_id: &str, model_index: usize) -> UnstampedToolCallOutcome {
    UnstampedToolCallOutcome {
        tool_use_id: tool_use_id.into(),
        tool_id: ToolId::Custom(tool_use_id.into()),
        model_index,
        ordered_messages: vec![],
        message_path: ToolMessagePath::Success,
        error_kind: None,
        permission_denial: None,
        prevent_continuation: None,
        structured_output: None,
        effects: ToolSideEffects::none(),
    }
}

fn unstamped_with_patch<F>(
    tool_use_id: &str,
    model_index: usize,
    patch: F,
) -> UnstampedToolCallOutcome
where
    F: FnOnce(&mut coco_types::ToolAppState) + Send + Sync + 'static,
{
    UnstampedToolCallOutcome {
        effects: ToolSideEffects {
            app_state_patch: Some(Box::new(patch)),
            permission_updates: Vec::new(),
        },
        ..empty_unstamped(tool_use_id, model_index)
    }
}

/// Drive `execute_with` and capture the completion-order sequence of
/// outcomes surfaced to `on_outcome`.
async fn drive_capture<F, Fut>(
    exec: &ToolExecutor,
    plans: Vec<ToolCallPlan>,
    run_one: F,
) -> Vec<ToolCallOutcome>
where
    F: Fn(PreparedToolCall, crate::call_plan::RunOneRuntime) -> Fut + Sync,
    Fut: std::future::Future<Output = UnstampedToolCallOutcome> + Send,
{
    let captured = Mutex::new(Vec::<ToolCallOutcome>::new());
    exec.execute_with(plans, run_one, |o| captured.lock().unwrap().push(o))
        .await;
    captured.into_inner().unwrap()
}

#[tokio::test]
async fn test_execute_with_concurrent_batch_surfaces_in_completion_order() {
    // Three safe tools: A sleeps 80ms, B sleeps 10ms, C sleeps 40ms.
    // Completion order MUST be [B, C, A] regardless of submission.
    let a = Arc::new(SlowSafeTool {
        name: "a".into(),
        concurrent_count: Arc::new(AtomicI32::new(0)),
        max_concurrent: Arc::new(AtomicI32::new(0)),
        sleep_ms: 80,
    });
    let b = Arc::new(SlowSafeTool {
        name: "b".into(),
        concurrent_count: Arc::new(AtomicI32::new(0)),
        max_concurrent: Arc::new(AtomicI32::new(0)),
        sleep_ms: 10,
    });
    let c = Arc::new(SlowSafeTool {
        name: "c".into(),
        concurrent_count: Arc::new(AtomicI32::new(0)),
        max_concurrent: Arc::new(AtomicI32::new(0)),
        sleep_ms: 40,
    });

    let plans = vec![
        ToolCallPlan::Runnable(prepared_from(a, "A", 0)),
        ToolCallPlan::Runnable(prepared_from(b, "B", 1)),
        ToolCallPlan::Runnable(prepared_from(c, "C", 2)),
    ];

    let exec = ToolExecutor::new();
    let outcomes = drive_capture(&exec, plans, |prepared, _runtime| async move {
        let tool_use_id = prepared.tool_use_id.clone();
        let model_index = prepared.model_index;
        // Simulate actual tool work via the SlowSafeTool's own sleep.
        let ctx = crate::context::ToolUseContext::test_default();
        let _ = prepared
            .tool
            .execute(prepared.parsed_input.into_value(), &ctx)
            .await;
        empty_unstamped(&tool_use_id, model_index)
    })
    .await;

    let ids: Vec<_> = outcomes
        .iter()
        .map(|o| o.tool_use_id().to_string())
        .collect();
    assert_eq!(
        ids,
        vec!["B".to_string(), "C".to_string(), "A".to_string()],
        "concurrent batch must surface in completion order (fast → slow)"
    );

    // completion_seq stamped monotonically in surface order.
    let seqs: Vec<_> = outcomes
        .iter()
        .map(ToolCallOutcome::completion_seq)
        .collect();
    assert_eq!(seqs, vec![0, 1, 2]);
}

#[tokio::test]
async fn test_execute_with_bash_failure_aborts_concurrent_sibling_runtime() {
    let bash_tool = Arc::new(SafeTool {
        name: "bash".into(),
    });
    let read_tool = Arc::new(SafeTool {
        name: "read".into(),
    });
    let plans = vec![
        ToolCallPlan::Runnable(PreparedToolCall {
            tool_use_id: "bash-call".into(),
            tool_id: ToolId::Builtin(coco_types::ToolName::Bash),
            parsed_input: crate::ValidatedInput::validate(bash_tool.as_ref(), json!({}))
                .expect("test input must validate"),
            is_concurrency_safe: true,
            tool: bash_tool,
            model_index: 0,
            permission_resolution_detail: None,
            approval_feedback: None,
        }),
        ToolCallPlan::Runnable(prepared_from(read_tool, "read-call", 1)),
    ];
    let observed_reason = Arc::new(Mutex::new(None));
    let observed_reason_for_run = observed_reason.clone();

    let exec = ToolExecutor::new();
    let outcomes = drive_capture(&exec, plans, move |prepared, runtime| {
        let observed_reason = observed_reason_for_run.clone();
        async move {
            if prepared.tool_use_id == "bash-call" {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let mut outcome = empty_unstamped(&prepared.tool_use_id, prepared.model_index);
                outcome.tool_id = ToolId::Builtin(coco_types::ToolName::Bash);
                outcome.message_path = ToolMessagePath::Failure;
                outcome.error_kind = Some(crate::call_plan::ToolCallErrorKind::ExecutionFailed);
                return outcome;
            }

            runtime.abort.cancelled().await;
            *observed_reason.lock().unwrap() = runtime.abort.reason();
            let mut outcome = empty_unstamped(&prepared.tool_use_id, prepared.model_index);
            outcome.message_path = ToolMessagePath::Failure;
            outcome.error_kind = Some(crate::call_plan::ToolCallErrorKind::ExecutionCancelled);
            outcome
        }
    })
    .await;

    assert_eq!(outcomes.len(), 2);
    assert!(matches!(
        observed_reason.lock().unwrap().as_ref(),
        Some(coco_types::ToolAbortReasonPayload::SiblingError { failed_tool })
            if failed_tool == coco_types::ToolName::Bash.as_str()
    ));
}

#[tokio::test]
async fn test_execute_with_concurrent_batch_applies_patches_in_model_order() {
    // Two concurrent tools; A's patch sets permission_mode → Plan,
    // B's patch sets permission_mode → Default. After the batch the
    // state must reflect the LAST write in model order (= B, index 1),
    // regardless of which future resolved first.
    let app_state = Arc::new(RwLock::new(coco_types::ToolAppState::default()));
    let exec = ToolExecutor::new().with_app_state(app_state.clone());

    let a = Arc::new(SafeTool { name: "a".into() });
    let b = Arc::new(SafeTool { name: "b".into() });

    let plans = vec![
        ToolCallPlan::Runnable(prepared_from(a, "A", 0)),
        ToolCallPlan::Runnable(prepared_from(b, "B", 1)),
    ];

    // Vary completion order: B finishes first (so completion_seq 0),
    // A finishes second (completion_seq 1). Model-order patches still
    // mean A applies before B — final state = B's write.
    drive_capture(&exec, plans, |prepared, _runtime| async move {
        let tool_use_id = prepared.tool_use_id.clone();
        let model_index = prepared.model_index;
        let sleep_ms = if tool_use_id == "A" { 40 } else { 5 };
        tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
        let target_mode = if model_index == 0 {
            coco_types::PermissionMode::Plan
        } else {
            coco_types::PermissionMode::Default
        };
        unstamped_with_patch(&tool_use_id, model_index, move |state| {
            state.permissions.mode = Some(target_mode);
        })
    })
    .await;

    let guard = app_state.read().await;
    assert_eq!(
        guard.permissions.mode,
        Some(coco_types::PermissionMode::Default),
        "post-batch state must reflect B (model_index 1) applied AFTER A, \
         regardless of completion order"
    );
}

#[tokio::test]
async fn test_execute_with_serial_tool_applies_patch_before_next_context() {
    // A serial unsafe tool followed by another serial unsafe tool.
    // The first tool's patch must be visible (via shared state read)
    // by the time the second run_one is called — serial mode applies
    // between tools.
    let app_state = Arc::new(RwLock::new(coco_types::ToolAppState::default()));
    let exec = ToolExecutor::new().with_app_state(app_state.clone());

    let u1 = Arc::new(UnsafeTool { name: "u1".into() });
    let u2 = Arc::new(UnsafeTool { name: "u2".into() });

    let plans = vec![
        ToolCallPlan::Runnable(prepared_from(u1, "u1", 0)),
        ToolCallPlan::Runnable(prepared_from(u2, "u2", 1)),
    ];

    // u1 writes has_exited_plan_mode = true. When u2's run_one
    // fires, we read shared state and capture what we saw.
    let observed_by_u2: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
    let observed_by_u2_clone = observed_by_u2.clone();
    let app_state_read = app_state.clone();

    exec.execute_with(
        plans,
        move |prepared, _runtime| {
            let app_state_read = app_state_read.clone();
            let observed = observed_by_u2_clone.clone();
            async move {
                let tool_use_id = prepared.tool_use_id.clone();
                let model_index = prepared.model_index;
                if tool_use_id == "u1" {
                    unstamped_with_patch(&tool_use_id, model_index, |state| {
                        state.has_exited_plan_mode = true;
                    })
                } else {
                    // u2 snapshots the state it sees. Must already
                    // reflect u1's patch.
                    let snap = app_state_read.read().await.has_exited_plan_mode;
                    *observed.lock().unwrap() = Some(snap);
                    empty_unstamped(&tool_use_id, model_index)
                }
            }
        },
        |_outcome| {},
    )
    .await;

    assert_eq!(
        *observed_by_u2.lock().unwrap(),
        Some(true),
        "u2 must observe u1's patch — serial tools apply between calls"
    );
}

#[tokio::test]
async fn test_execute_with_early_outcome_is_barrier_between_safe_batches() {
    // Partition [safe_A, early_B, safe_C]:
    //   block 0: ConcurrentSafe([safe_A])
    //   block 1: EarlyOutcome(early_B)
    //   block 2: ConcurrentSafe([safe_C])
    //
    // EarlyOutcome must NOT share a batch with safe neighbors, and
    // its completion_seq lands between the two safe outcomes'
    // completion_seqs in partition order — NOT globally before both
    // (plan I12).
    let a = Arc::new(SafeTool { name: "a".into() });
    let c = Arc::new(SafeTool { name: "c".into() });

    let plans = vec![
        ToolCallPlan::Runnable(prepared_from(a, "A", 0)),
        ToolCallPlan::EarlyOutcome(UnstampedToolCallOutcome {
            tool_use_id: "B".into(),
            tool_id: ToolId::Custom("unknown".into()),
            model_index: 1,
            ordered_messages: vec![],
            message_path: ToolMessagePath::EarlyReturn,
            error_kind: Some(crate::call_plan::ToolCallErrorKind::UnknownTool),
            permission_denial: None,
            prevent_continuation: None,
            structured_output: None,
            effects: ToolSideEffects::none(),
        }),
        ToolCallPlan::Runnable(prepared_from(c, "C", 2)),
    ];

    let exec = ToolExecutor::new();
    let outcomes = drive_capture(&exec, plans, |prepared, _| async move {
        let tool_use_id = prepared.tool_use_id.clone();
        empty_unstamped(&tool_use_id, prepared.model_index)
    })
    .await;

    // Partition order: A, B, C — each in its own block.
    let ids: Vec<_> = outcomes
        .iter()
        .map(|o| o.tool_use_id().to_string())
        .collect();
    assert_eq!(ids, vec!["A", "B", "C"]);

    // completion_seq stamped in partition traversal order, not
    // "EarlyOutcome stamps first".
    let seqs: Vec<_> = outcomes
        .iter()
        .map(ToolCallOutcome::completion_seq)
        .collect();
    assert_eq!(seqs, vec![0, 1, 2]);

    // The middle outcome is the synthetic unknown-tool error.
    assert_eq!(
        outcomes[1].error_kind(),
        Some(&crate::call_plan::ToolCallErrorKind::UnknownTool)
    );
    assert_eq!(outcomes[1].message_path(), ToolMessagePath::EarlyReturn);
}

#[tokio::test]
async fn test_execute_with_every_plan_gets_one_outcome() {
    // I1 / per-call invariant: one ToolUseQueued → exactly one
    // ToolUseCompleted. Encoded here as: every plan submitted
    // surfaces exactly one outcome.
    let a = Arc::new(SafeTool { name: "a".into() });
    let b = Arc::new(UnsafeTool { name: "b".into() });

    let plans = vec![
        ToolCallPlan::Runnable(prepared_from(a, "A", 0)),
        ToolCallPlan::EarlyOutcome(empty_unstamped("B", 1)),
        ToolCallPlan::Runnable(prepared_from(b, "C", 2)),
    ];

    let exec = ToolExecutor::new();
    let outcomes = drive_capture(&exec, plans, |prepared, _| async move {
        empty_unstamped(&prepared.tool_use_id, prepared.model_index)
    })
    .await;

    assert_eq!(outcomes.len(), 3);
    let ids: Vec<_> = outcomes
        .iter()
        .map(|o| o.tool_use_id().to_string())
        .collect();
    assert_eq!(ids, vec!["A", "B", "C"]);
}

#[test]
fn resolve_max_concurrency_zero_falls_back_to_default() {
    // 0 falls back to default (would otherwise build a
    // 0-permit Semaphore and deadlock every concurrent-safe tool).
    assert_eq!(
        resolve_max_concurrency(Some("0".to_string())),
        DEFAULT_MAX_CONCURRENCY
    );
}

#[test]
fn resolve_max_concurrency_invalid_or_unset_falls_back_to_default() {
    assert_eq!(
        resolve_max_concurrency(Some("abc".to_string())),
        DEFAULT_MAX_CONCURRENCY
    );
    assert_eq!(resolve_max_concurrency(None), DEFAULT_MAX_CONCURRENCY);
}

#[test]
fn resolve_max_concurrency_valid_positive_is_honored() {
    assert_eq!(resolve_max_concurrency(Some("3".to_string())), 3);
}
