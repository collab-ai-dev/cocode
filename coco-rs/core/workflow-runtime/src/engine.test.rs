use std::collections::HashMap;
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

#[async_trait::async_trait(?Send)]
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

#[async_trait::async_trait(?Send)]
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
            /*depth*/ 0,
        )
        .await
    })
}

/// A host whose `run_agent` sleeps `delay`, simulating a long-running subagent.
/// Used to prove the sync-eval budget is NOT a whole-run wall clock: a run whose
/// async agent time exceeds the budget still completes.
struct SlowHost {
    delay: Duration,
}

#[async_trait::async_trait(?Send)]
impl WorkflowHost for SlowHost {
    async fn run_agent(
        &self,
        prompt: String,
        _opts: WorkflowAgentOpts,
    ) -> Result<WorkflowAgentResult, String> {
        tokio::time::sleep(self.delay).await;
        Ok(WorkflowAgentResult {
            value: serde_json::Value::String(format!("ran: {prompt}")),
            model: None,
            tokens: Some(1),
            tool_calls: Some(0),
            duration_ms: Some(self.delay.as_millis() as i64),
        })
    }

    fn push_progress(&self, _event: WorkflowProgressEvent) {}
}

/// Drive the engine with a SHORT sync-eval budget but a long-running async
/// agent: the run must complete rather than being killed when the budget
/// elapses, confirming the budget caps only synchronous evaluation chunks.
fn run_with_budget(
    host: Arc<dyn WorkflowHost>,
    script: &str,
    sync_eval_budget: Duration,
) -> Result<serde_json::Value, crate::WorkflowRuntimeError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        WorkflowEngine::run(
            script.to_string(),
            serde_json::Value::Null,
            host,
            CancellationToken::new(),
            sync_eval_budget,
            /*depth*/ 0,
        )
        .await
    })
}

#[test]
fn long_async_run_is_not_killed_when_sync_budget_elapses() {
    // Agent sleeps 200ms per call across two awaits (400ms total async time),
    // far exceeding the 40ms sync-eval budget. There is NO whole-run wall
    // clock, so the run completes — the budget caps only synchronous chunks,
    // which are reset at each host-callback boundary.
    let host = Arc::new(SlowHost {
        delay: Duration::from_millis(200),
    });
    let script = r#"
        const a = await agent("one");
        const b = await agent("two");
        return { a, b };
    "#;
    let result = run_with_budget(host, script, Duration::from_millis(40)).expect("run completes");
    assert_eq!(result["a"], serde_json::json!("ran: one"));
    assert_eq!(result["b"], serde_json::json!("ran: two"));
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
            /*depth*/ 0,
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

/// A host whose resume cache is pre-seeded: `cached_agent_result` returns a hit
/// for any prompt in `cache` (keyed on the prompt for test simplicity) and
/// counts every `run_agent` spawn so a test can assert a hit skips the spawn.
struct CacheHost {
    /// prompt → cached value to replay (a hit).
    cache: Mutex<std::collections::HashMap<String, serde_json::Value>>,
    /// Number of `run_agent` calls actually issued (a real spawn).
    spawns: AtomicI64,
}

impl CacheHost {
    fn new() -> Self {
        Self {
            cache: Mutex::new(std::collections::HashMap::new()),
            spawns: AtomicI64::new(0),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl WorkflowHost for CacheHost {
    async fn run_agent(
        &self,
        prompt: String,
        _opts: WorkflowAgentOpts,
    ) -> Result<WorkflowAgentResult, String> {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        Ok(WorkflowAgentResult {
            value: serde_json::Value::String(format!("ran: {prompt}")),
            model: None,
            tokens: Some(1),
            tool_calls: Some(0),
            duration_ms: Some(1),
        })
    }

    fn push_progress(&self, _event: WorkflowProgressEvent) {}

    async fn cached_agent_result(&self, key: &crate::AgentCacheKey) -> Option<serde_json::Value> {
        self.cache.lock().unwrap().get(&key.prompt).cloned()
    }
}

#[test]
fn cache_hit_skips_spawn_and_emits_cached_true() {
    let host = Arc::new(CacheHost::new());
    host.cache
        .lock()
        .unwrap()
        .insert("one".to_string(), serde_json::json!("replayed-one"));
    // FakeHost-style progress capture: wrap CacheHost progress by recording.
    // We assert via the returned value + spawn count + a dedicated progress sink.
    let progress: Arc<Mutex<Vec<WorkflowProgressEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_host = Arc::new(ProgressTee {
        inner: host.clone(),
        progress: progress.clone(),
    });
    let script = r#"return await agent("one");"#;
    let result = run_with_host(sink_host, script, serde_json::Value::Null).expect("run completes");
    assert_eq!(result, serde_json::json!("replayed-one"));
    // No spawn happened — the result came from the cache.
    assert_eq!(host.spawns.load(Ordering::SeqCst), 0);
    // A single Done event with cached:true, and NO Start event.
    let events = progress.lock().unwrap();
    assert!(events.iter().any(|e| matches!(
        e,
        WorkflowProgressEvent::WorkflowAgent {
            state: WorkflowAgentState::Done,
            cached: true,
            ..
        }
    )));
    assert!(!events.iter().any(|e| matches!(
        e,
        WorkflowProgressEvent::WorkflowAgent {
            state: WorkflowAgentState::Start,
            ..
        }
    )));
}

#[test]
fn first_miss_diverges_so_a_later_matching_key_is_not_cached() {
    // Cache holds entries for BOTH "one" and "two", but "one" is called first
    // and MISSES (not in cache here), flipping diverged=true. After divergence,
    // "two" is NOT served from cache even though a hit exists — it re-spawns.
    let host = Arc::new(CacheHost::new());
    host.cache
        .lock()
        .unwrap()
        .insert("two".to_string(), serde_json::json!("replayed-two"));
    let script = r#"
        const a = await agent("one"); // miss → diverges
        const b = await agent("two"); // would hit, but cursor diverged → spawns
        return { a, b };
    "#;
    let result = run_with_host(host.clone(), script, serde_json::Value::Null).expect("run");
    // "one" missed (spawned), "two" was NOT served from cache (spawned too).
    assert_eq!(result["a"], serde_json::json!("ran: one"));
    assert_eq!(result["b"], serde_json::json!("ran: two"));
    assert_eq!(host.spawns.load(Ordering::SeqCst), 2);
}

/// A pass-through host that tees every progress event into a shared vec while
/// delegating all real behavior (including the resume cache) to `inner`.
struct ProgressTee {
    inner: Arc<CacheHost>,
    progress: Arc<Mutex<Vec<WorkflowProgressEvent>>>,
}

#[async_trait::async_trait(?Send)]
impl WorkflowHost for ProgressTee {
    async fn run_agent(
        &self,
        prompt: String,
        opts: WorkflowAgentOpts,
    ) -> Result<WorkflowAgentResult, String> {
        self.inner.run_agent(prompt, opts).await
    }

    fn push_progress(&self, event: WorkflowProgressEvent) {
        self.progress.lock().unwrap().push(event);
    }

    async fn cached_agent_result(&self, key: &crate::AgentCacheKey) -> Option<serde_json::Value> {
        self.inner.cached_agent_result(key).await
    }
}

/// A host that supports nested `workflow(name, args)` by re-entering the engine
/// with the SAME host Arc (`me`), exactly as `WorkflowRunHost` does. Child scripts
/// are looked up by name from `scripts`; an unknown name returns `Err` so the
/// parent's `workflow()` rejects. Tracks max concurrent `run_agent` calls across
/// parent + child through a shared semaphore + counter to prove governance is
/// shared (not freshly allocated per nested run).
struct NestedHost {
    me: std::sync::Weak<dyn WorkflowHost>,
    scripts: HashMap<String, String>,
    semaphore: Arc<tokio::sync::Semaphore>,
    in_flight: AtomicI64,
    max_in_flight: AtomicI64,
    spawns: AtomicI64,
}

impl NestedHost {
    fn new(cap: usize, scripts: HashMap<String, String>) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            me: me.clone() as std::sync::Weak<dyn WorkflowHost>,
            scripts,
            semaphore: Arc::new(tokio::sync::Semaphore::new(cap)),
            in_flight: AtomicI64::new(0),
            max_in_flight: AtomicI64::new(0),
            spawns: AtomicI64::new(0),
        })
    }
}

#[async_trait::async_trait(?Send)]
impl WorkflowHost for NestedHost {
    async fn run_agent(
        &self,
        prompt: String,
        _opts: WorkflowAgentOpts,
    ) -> Result<WorkflowAgentResult, String> {
        // Shared admission gate: both parent and child agents queue here. If the
        // child allocated its own governance, more than `cap` could run at once.
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| format!("semaphore closed: {e}"))?;
        self.spawns.fetch_add(1, Ordering::SeqCst);
        let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(current, Ordering::SeqCst);
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

    async fn run_nested_workflow(
        &self,
        name_or_ref: String,
        args: serde_json::Value,
        depth: i32,
    ) -> Result<serde_json::Value, String> {
        let body = self
            .scripts
            .get(&name_or_ref)
            .cloned()
            .ok_or_else(|| format!("workflow '{name_or_ref}' was not found"))?;
        // Re-enter on THIS thread with the SAME host (governance shared); the
        // child runs at depth>=1 so its own workflow() throws.
        let host = self.me.upgrade().expect("host alive");
        WorkflowEngine::run(
            body,
            args,
            host,
            CancellationToken::new(),
            Duration::from_secs(10),
            depth,
        )
        .await
        .map_err(|error| error.to_string())
    }
}

#[test]
fn nested_workflow_runs_child_and_returns_its_result() {
    let mut scripts = HashMap::new();
    scripts.insert(
        "child".to_string(),
        r#"const r = await agent("child-step"); return { from: "child", r };"#.to_string(),
    );
    let host = NestedHost::new(4, scripts);
    let script = r#"const c = await workflow("child", { k: 1 }); return { c, parent: true };"#;
    let result = run_with_host(host.clone(), script, serde_json::Value::Null).expect("run");
    assert_eq!(result["parent"], serde_json::json!(true));
    assert_eq!(result["c"]["from"], serde_json::json!("child"));
    assert_eq!(result["c"]["r"], serde_json::json!("ran: child-step"));
    // The child's agent spawn went through the shared host.
    assert_eq!(host.spawns.load(Ordering::SeqCst), 1);
}

#[test]
fn nested_child_workflow_call_rejects_with_one_level_error() {
    // The child itself calls workflow() — the one-level guard must reject it. The
    // child catches it and surfaces the message, proving the guard fired.
    let mut scripts = HashMap::new();
    scripts.insert(
        "child".to_string(),
        r#"try { await workflow("grandchild"); return "should not reach"; }
           catch (e) { return String(e.message || e); }"#
            .to_string(),
    );
    scripts.insert("grandchild".to_string(), r#"return "gc";"#.to_string());
    let host = NestedHost::new(4, scripts);
    let script = r#"return await workflow("child");"#;
    let result = run_with_host(host, script, serde_json::Value::Null).expect("run");
    assert_eq!(
        result,
        serde_json::json!(super::WORKFLOW_NESTING_LIMIT_ERROR)
    );
}

#[test]
fn unknown_nested_workflow_name_rejects_to_parent() {
    // workflow() on an unknown name returns Err from the host → the JS workflow()
    // rejects; the parent catches it.
    let host = NestedHost::new(4, HashMap::new());
    let script = r#"try { await workflow("nope"); return "unreachable"; }
                    catch (e) { return "caught: " + String(e.message || e); }"#;
    let result = run_with_host(host, script, serde_json::Value::Null).expect("run");
    let text = result.as_str().expect("string result");
    assert!(text.starts_with("caught: "), "got: {text}");
    assert!(text.contains("was not found"), "got: {text}");
}

#[test]
fn nested_agents_share_parent_concurrency_cap() {
    // The child fans out many agents in parallel; combined with the parent's own
    // agents they all queue on the SAME semaphore (cap 2). If the child allocated
    // fresh governance, max-in-flight would exceed the cap.
    let cap = 2usize;
    let mut scripts = HashMap::new();
    scripts.insert(
        "child".to_string(),
        r#"const fs = [];
           for (let i = 0; i < 8; i++) fs.push(() => agent("c" + i));
           return await parallel(fs);"#
            .to_string(),
    );
    let host = NestedHost::new(cap, scripts);
    let script = r#"
        const fs = [];
        for (let i = 0; i < 8; i++) fs.push(() => agent("p" + i));
        // Parent agents and the nested child (which itself fans out) run together.
        const [parent, child] = await Promise.all([parallel(fs), workflow("child")]);
        return { parent, child };
    "#;
    let result = run_with_host(host.clone(), script, serde_json::Value::Null).expect("run");
    assert_eq!(result["parent"].as_array().expect("parent arr").len(), 8);
    assert_eq!(result["child"].as_array().expect("child arr").len(), 8);
    // 8 parent + 8 child agent spawns, all through the shared host.
    assert_eq!(host.spawns.load(Ordering::SeqCst), 16);
    let max = host.max_in_flight.load(Ordering::SeqCst);
    assert!(max >= 1, "at least one agent ran");
    assert!(
        max <= cap as i64,
        "max in-flight {max} exceeded the SHARED semaphore cap {cap} — child allocated its own governance"
    );
}
