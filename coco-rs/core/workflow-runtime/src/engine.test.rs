use std::sync::Arc;
use std::sync::Mutex;
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
        let value = match opts.model {
            Some(model) => serde_json::json!({ "prompt": prompt, "model": model }),
            None => serde_json::Value::String(format!("ran: {prompt}")),
        };
        Ok(WorkflowAgentResult { value })
    }

    fn push_progress(&self, event: WorkflowProgressEvent) {
        self.progress.lock().unwrap().push(event);
    }
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
