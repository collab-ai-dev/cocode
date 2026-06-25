use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use coco_types::WorkflowAgentState;
use coco_types::WorkflowProgressEvent;
use tokio_util::sync::CancellationToken;

use super::WorkflowEngine;
use crate::host::WorkflowAgentOpts;
use crate::host::WorkflowAgentResult;
use crate::host::WorkflowHost;

#[derive(Default)]
struct FakeHost {
    progress: Mutex<Vec<WorkflowProgressEvent>>,
    /// Prompts that should make `run_agent` fail (to exercise rejection).
    fail_on: Mutex<Vec<String>>,
    budget_total: Option<i64>,
    budget_spent: AtomicI64,
}

#[async_trait::async_trait]
impl WorkflowHost for FakeHost {
    async fn run_agent(
        &self,
        prompt: String,
        opts: WorkflowAgentOpts,
    ) -> Result<WorkflowAgentResult, String> {
        if self.fail_on.lock().unwrap().contains(&prompt) {
            return Err(format!("boom: {prompt}"));
        }
        let value = match opts.model.as_ref() {
            Some(model) => serde_json::json!({ "prompt": prompt, "model": model }),
            None => serde_json::Value::String(format!("ran: {prompt}")),
        };
        let model = opts.model;
        Ok(WorkflowAgentResult {
            value,
            model,
            tokens: Some(10),
            tool_calls: Some(1),
            duration_ms: Some(25),
        })
    }

    fn push_progress(&self, event: WorkflowProgressEvent) {
        self.progress.lock().unwrap().push(event);
    }

    fn budget_total_tokens(&self) -> Option<i64> {
        self.budget_total
    }

    fn budget_spent_tokens(&self) -> i64 {
        self.budget_spent.load(Ordering::Relaxed)
    }

    fn record_agent_tokens(&self, tokens: i64) {
        self.budget_spent.fetch_add(tokens, Ordering::Relaxed);
    }

    fn budget_exhausted(&self) -> bool {
        self.budget_total
            .is_some_and(|total| total > 0 && self.budget_spent_tokens() >= total)
    }
}

/// A host that records the maximum number of concurrently in-flight `run_agent`
/// calls and serializes admission through a semaphore of a fixed `cap`, exactly
/// mirroring `WorkflowRunHost`'s host-side concurrency gate.
struct ConcurrencyHost {
    semaphore: Arc<tokio::sync::Semaphore>,
    in_flight: AtomicI64,
    max_in_flight: AtomicI64,
}

impl ConcurrencyHost {
    fn new(cap: usize) -> Self {
        Self {
            semaphore: Arc::new(tokio::sync::Semaphore::new(cap)),
            in_flight: AtomicI64::new(0),
            max_in_flight: AtomicI64::new(0),
        }
    }
}

#[async_trait::async_trait]
impl WorkflowHost for ConcurrencyHost {
    async fn run_agent(
        &self,
        prompt: String,
        _opts: WorkflowAgentOpts,
    ) -> Result<WorkflowAgentResult, String> {
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| format!("semaphore closed: {e}"))?;
        let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(current, Ordering::SeqCst);
        // Yield so concurrent admissions interleave before we release.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(WorkflowAgentResult {
            value: serde_json::Value::String(format!("ran: {prompt}")),
            model: None,
            tokens: Some(1),
            tool_calls: Some(0),
            duration_ms: Some(1),
        })
    }

    fn push_progress(&self, _event: WorkflowProgressEvent) {}
}

fn run(
    host: Arc<FakeHost>,
    script: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, crate::WorkflowRuntimeError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        WorkflowEngine::run(
            script.to_string(),
            args,
            host,
            CancellationToken::new(),
            Duration::from_secs(10),
        )
        .await
    })
}

#[test]
fn runs_dsl_script_with_agent_parallel_phase_log_and_args() {
    let host = Arc::new(FakeHost::default());
    let script = r#"
        phase("Plan");
        log("starting");
        const a = await agent("do x");
        const both = await parallel([() => agent("p1"), () => agent("p2")]);
        return { a, both, k: args.k };
    "#;
    let result = run(host.clone(), script, serde_json::json!({ "k": 42 })).expect("run");
    assert_eq!(result["a"], serde_json::json!("ran: do x"));
    assert_eq!(result["both"], serde_json::json!(["ran: p1", "ran: p2"]));
    assert_eq!(result["k"], serde_json::json!(42));

    let progress = host.progress.lock().unwrap();
    assert!(
        progress
            .iter()
            .any(|e| matches!(e, WorkflowProgressEvent::WorkflowPhase { .. }))
    );
    assert!(
        progress
            .iter()
            .any(|e| matches!(e, WorkflowProgressEvent::WorkflowLog { .. }))
    );
    // 3 agent calls → 3 Start + 3 Done.
    let starts = progress
        .iter()
        .filter(|e| {
            matches!(
                e,
                WorkflowProgressEvent::WorkflowAgent {
                    state: WorkflowAgentState::Start,
                    ..
                }
            )
        })
        .count();
    let done = progress
        .iter()
        .filter(|e| {
            matches!(
                e,
                WorkflowProgressEvent::WorkflowAgent {
                    state: WorkflowAgentState::Done,
                    ..
                }
            )
        })
        .count();
    assert_eq!(starts, 3);
    assert_eq!(done, 3);
}

#[test]
fn per_call_model_opt_reaches_host() {
    let host = Arc::new(FakeHost::default());
    let script = r#"return await agent("hi", { model: "claude-opus-4-8" });"#;
    let result = run(host, script, serde_json::Value::Null).expect("run");
    assert_eq!(result["model"], serde_json::json!("claude-opus-4-8"));
}

#[test]
fn budget_reflects_child_agent_token_usage() {
    let host = Arc::new(FakeHost {
        budget_total: Some(25),
        ..FakeHost::default()
    });
    let script = r#"
        const before = budget.remaining();
        await agent("one");
        const afterOne = budget.remaining();
        await agent("two");
        return { total: budget.total, spent: budget.spent(), before, afterOne, afterTwo: budget.remaining() };
    "#;
    let result = run(host, script, serde_json::Value::Null).expect("run");
    assert_eq!(result["total"], serde_json::json!(25));
    assert_eq!(result["spent"], serde_json::json!(20));
    assert_eq!(result["before"], serde_json::json!(25));
    assert_eq!(result["afterOne"], serde_json::json!(15));
    assert_eq!(result["afterTwo"], serde_json::json!(5));
}

#[test]
fn failing_agent_in_parallel_becomes_null_not_a_crash() {
    let host = Arc::new(FakeHost::default());
    host.fail_on.lock().unwrap().push("bad".to_string());
    let script = r#"return await parallel([() => agent("good"), () => agent("bad")]);"#;
    let result = run(host.clone(), script, serde_json::Value::Null).expect("run");
    assert_eq!(result, serde_json::json!(["ran: good", null]));
    // The failed agent emitted an Error progress event.
    assert!(host.progress.lock().unwrap().iter().any(|e| matches!(
        e,
        WorkflowProgressEvent::WorkflowAgent {
            state: WorkflowAgentState::Error,
            ..
        }
    )));
}

#[test]
fn determinism_shim_applies_inside_the_engine() {
    let host = Arc::new(FakeHost::default());
    let err =
        run(host, "return Math.random();", serde_json::Value::Null).expect_err("should reject");
    assert!(matches!(err, crate::WorkflowRuntimeError::Script { .. }));
}

#[test]
fn nondeterministic_static_aside_runtime_blocks_date_now() {
    let host = Arc::new(FakeHost::default());
    let err = run(host, "return Date.now();", serde_json::Value::Null).expect_err("should reject");
    assert!(matches!(err, crate::WorkflowRuntimeError::Script { .. }));
}

/// Run the engine against any host (not just `FakeHost`).
fn run_with_host(
    host: Arc<dyn WorkflowHost>,
    script: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, crate::WorkflowRuntimeError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        WorkflowEngine::run(
            script.to_string(),
            args,
            host,
            CancellationToken::new(),
            Duration::from_secs(10),
        )
        .await
    })
}

#[test]
fn parallel_admission_never_exceeds_semaphore_cap() {
    let cap = 3usize;
    let host = Arc::new(ConcurrencyHost::new(cap));
    // 20 thunks, far more than the cap of 3. parallel() fires all thunks; the
    // host-side semaphore must keep concurrent run_agent calls <= cap.
    let script = r#"
        const funcs = [];
        for (let i = 0; i < 20; i++) funcs.push(() => agent("p" + i));
        return await parallel(funcs);
    "#;
    let result = run_with_host(host.clone(), script, serde_json::Value::Null).expect("run");
    let arr = result.as_array().expect("array result");
    assert_eq!(arr.len(), 20);
    assert!(arr.iter().all(serde_json::Value::is_string));
    let max = host.max_in_flight.load(Ordering::SeqCst);
    assert!(max >= 1, "at least one agent should have run");
    assert!(
        max <= cap as i64,
        "max in-flight {max} exceeded semaphore cap {cap}"
    );
}

#[test]
fn agent_call_at_or_over_budget_rejects_to_null_in_parallel() {
    // total = 5, spent = 5 → budget_exhausted() is true, so every agent()
    // throws before spawning, degrading each parallel slot to null.
    let host = Arc::new(FakeHost {
        budget_total: Some(5),
        ..FakeHost::default()
    });
    host.budget_spent.store(5, Ordering::Relaxed);
    let script = r#"return await parallel([() => agent("a"), () => agent("b")]);"#;
    let result = run_with_host(host, script, serde_json::Value::Null).expect("run");
    assert_eq!(result, serde_json::json!([null, null]));
}

#[test]
fn budget_exhausts_mid_run_and_later_agent_rejects() {
    // total = 15, each agent spends 10. First agent succeeds (spent 0 < 15),
    // raising spent to 10; second agent succeeds (10 < 15) raising spent to 20;
    // third agent is gated (20 >= 15) and rejects → null in the parallel batch.
    let host = Arc::new(FakeHost {
        budget_total: Some(15),
        ..FakeHost::default()
    });
    let script = r#"
        await agent("first");
        await agent("second");
        return await parallel([() => agent("third")]);
    "#;
    let result = run_with_host(host, script, serde_json::Value::Null).expect("run");
    assert_eq!(result, serde_json::json!([null]));
}

#[test]
fn parallel_rejects_oversized_array() {
    let host = Arc::new(FakeHost::default());
    // WORKFLOW_ARRAY_CAP + 1 thunks → RangeError thrown synchronously.
    let script = r#"
        const funcs = [];
        for (let i = 0; i <= 4096; i++) funcs.push(() => agent("x"));
        return await parallel(funcs);
    "#;
    let err = run_with_host(host, script, serde_json::Value::Null).expect_err("should reject");
    assert!(matches!(err, crate::WorkflowRuntimeError::Script { .. }));
}
