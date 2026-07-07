use std::sync::Arc;

use coco_tool_runtime::NoOpAgentHandle;
use coco_tool_runtime::NoOpBackgroundTaskHandle;
use coco_workflow_runtime::WorkflowAgentOpts;

use super::WorkflowRunHost;
use super::WorkflowSpawnContext;

/// A throwaway per-attempt abort signal for tests that call `build_request`
/// directly (production threads a fresh child-token signal per attempt).
fn test_abort() -> coco_tool_runtime::TurnAbortSignal {
    coco_tool_runtime::TurnAbortSignal::from_token(tokio_util::sync::CancellationToken::new())
}

fn host() -> WorkflowRunHost {
    WorkflowRunHost {
        agent: Arc::new(NoOpAgentHandle),
        task_handle: Arc::new(NoOpBackgroundTaskHandle),
        task_id: "wtest".to_string(),
        main_handle: tokio::runtime::Handle::current(),
        spawn_ctx: WorkflowSpawnContext {
            session_id: Some(coco_types::SessionId::try_new("session").unwrap()),
            invoking_agent_id: Some("parent-agent".to_string()),
            tool_use_id: Some("toolu_1".to_string()),
            features: Arc::new(coco_types::Features::with_defaults()),
            skill_overrides: Arc::new(coco_config::SkillOverrideTiers::default()),
            tool_overrides: Arc::new(coco_types::ToolOverrides::none()),
            parent_tool_filter: coco_types::ToolFilter::unrestricted(),
            active_shell_tool: coco_types::ActiveShellTool::Disabled,
            log_assistant_responses: None,
            parent_mode: coco_types::PermissionMode::Default,
            agent_catalog: None,
            total_token_budget: Some(100),
            workflow_abort: coco_tool_runtime::TurnAbortSignal::from_token(
                tokio_util::sync::CancellationToken::new(),
            ),
            cwd: None,
        },
        budget_spent_tokens: std::sync::atomic::AtomicI64::new(0),
        semaphore: Arc::new(tokio::sync::Semaphore::new(
            super::workflow_local_concurrency(),
        )),
        journal: Arc::new(super::WorkflowJournal::new(None)),
        // Tests construct the host directly (not via `Arc::new_cyclic`), and none
        // exercise nested `workflow()`, so a dangling weak self-ref is fine.
        me: std::sync::Weak::<super::WorkflowRunHost>::new(),
    }
}

#[test]
fn workflow_local_concurrency_within_floor_and_ceiling() {
    let width = super::workflow_local_concurrency();
    assert!(width >= super::WORKFLOW_CONCURRENCY_FLOOR);
    assert!(width <= super::WORKFLOW_CONCURRENCY_CEILING);
}

#[test]
fn local_workflow_runtime_drives_not_send_future() {
    let runtime = super::LocalWorkflowRuntime::new().expect("local workflow runtime");
    runtime.block_on(super::LocalOnlyReady::new());
}

#[tokio::test]
async fn budget_exhausted_reflects_total_and_spent() {
    use coco_workflow_runtime::WorkflowHost;
    // total = Some(100), spent = 0 → not exhausted.
    let host = host();
    assert!(!host.budget_exhausted());
    // Spend up to the total → exhausted.
    host.record_agent_tokens(100);
    assert!(host.budget_exhausted());
}

#[tokio::test]
async fn build_request_synthesizes_definition_for_workflow_overrides() {
    let host = host();
    let request = host
        .build_request(
            "research".to_string(),
            &WorkflowAgentOpts {
                agent_type: Some("Explore".to_string()),
                model: Some("anthropic/custom-model".to_string()),
                effort: Some("high".to_string()),
                isolation: Some(coco_types::AgentIsolation::Worktree),
                ..WorkflowAgentOpts::default()
            },
            test_abort(),
        )
        .expect("request");

    assert_eq!(request.input.subagent_type.as_deref(), Some("Explore"));
    assert_eq!(
        request.execution.isolation,
        Some(coco_types::AgentIsolation::Worktree)
    );
    let definition = request.input.definition.expect("synthetic definition");
    assert_eq!(definition.name, "Explore");
    assert_eq!(definition.model.as_deref(), Some("anthropic/custom-model"));
    assert_eq!(definition.effort, Some(coco_types::ReasoningEffort::High));
    assert_eq!(definition.isolation, coco_types::AgentIsolation::Worktree);
    assert!(request.routing.parent_turn_abort.is_some());
}

#[tokio::test]
async fn build_request_carries_output_schema_when_schema_present() {
    let host = host();
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "answer": { "type": "string" } },
        "required": ["answer"]
    });
    let request = host
        .build_request(
            "research".to_string(),
            &WorkflowAgentOpts {
                schema: Some(schema.clone()),
                ..WorkflowAgentOpts::default()
            },
            test_abort(),
        )
        .expect("request");
    let carried = request.input.output_schema.expect("output_schema is Some");
    assert_eq!(*carried.as_ref(), schema);
}

#[tokio::test]
async fn build_request_omits_output_schema_when_absent() {
    let host = host();
    let request = host
        .build_request(
            "research".to_string(),
            &WorkflowAgentOpts::default(),
            test_abort(),
        )
        .expect("request");
    assert!(request.input.output_schema.is_none());
}

#[tokio::test]
async fn run_agent_uses_captured_structured_output() {
    use coco_tool_runtime::AgentHandle;
    use coco_tool_runtime::AgentSpawnRequest;
    use coco_tool_runtime::AgentSpawnResponse;
    use coco_tool_runtime::AgentSpawnStatus;
    use coco_tool_runtime::TeamMessageDispatchResult;
    use coco_workflow_runtime::WorkflowHost;

    // Test handle whose spawn surfaces a captured `structured_output`
    // (the StructuredOutput tool-call input) AND a free-form result text
    // that would parse to a DIFFERENT value — proving the host prefers the
    // captured structured value over re-parsing the text.
    #[derive(Debug)]
    struct StructuredHandle;

    #[async_trait::async_trait]
    impl AgentHandle for StructuredHandle {
        async fn spawn_agent(
            &self,
            _request: AgentSpawnRequest,
        ) -> Result<AgentSpawnResponse, String> {
            Ok(AgentSpawnResponse {
                status: AgentSpawnStatus::Completed,
                result: Some("\"text-fallback-value\"".to_string()),
                structured_output: Some(serde_json::json!({ "answer": "from-tool" })),
                ..Default::default()
            })
        }
        async fn send_message(
            &self,
            _to: &str,
            _content: &str,
            _summary: Option<&str>,
        ) -> Result<TeamMessageDispatchResult, String> {
            Err("unused".into())
        }
        async fn query_agent_status(&self, _agent_id: &str) -> Result<AgentSpawnResponse, String> {
            Err("unused".into())
        }
        async fn get_agent_output(&self, _agent_id: &str) -> Result<String, String> {
            Err("unused".into())
        }
    }

    let mut host = host();
    host.agent = Arc::new(StructuredHandle);
    let result = host
        .run_agent(
            "compute".to_string(),
            WorkflowAgentOpts {
                schema: Some(serde_json::json!({ "type": "object" })),
                ..WorkflowAgentOpts::default()
            },
        )
        .await
        .expect("run_agent ok");
    // The captured StructuredOutput tool-call input wins over the text parse.
    assert_eq!(result.value, serde_json::json!({ "answer": "from-tool" }));
}

#[tokio::test]
async fn run_agent_schema_errors_without_capture() {
    use coco_tool_runtime::AgentHandle;
    use coco_tool_runtime::AgentSpawnRequest;
    use coco_tool_runtime::AgentSpawnResponse;
    use coco_tool_runtime::AgentSpawnStatus;
    use coco_tool_runtime::TeamMessageDispatchResult;
    use coco_workflow_runtime::WorkflowHost;

    // No captured structured value in schema mode is a contract failure.
    #[derive(Debug)]
    struct TextOnlyHandle;

    #[async_trait::async_trait]
    impl AgentHandle for TextOnlyHandle {
        async fn spawn_agent(
            &self,
            _request: AgentSpawnRequest,
        ) -> Result<AgentSpawnResponse, String> {
            Ok(AgentSpawnResponse {
                status: AgentSpawnStatus::Completed,
                result: Some("{\"answer\":\"parsed\"}".to_string()),
                structured_output: None,
                ..Default::default()
            })
        }
        async fn send_message(
            &self,
            _to: &str,
            _content: &str,
            _summary: Option<&str>,
        ) -> Result<TeamMessageDispatchResult, String> {
            Err("unused".into())
        }
        async fn query_agent_status(&self, _agent_id: &str) -> Result<AgentSpawnResponse, String> {
            Err("unused".into())
        }
        async fn get_agent_output(&self, _agent_id: &str) -> Result<String, String> {
            Err("unused".into())
        }
    }

    let mut host = host();
    host.agent = Arc::new(TextOnlyHandle);
    let err = host
        .run_agent(
            "compute".to_string(),
            WorkflowAgentOpts {
                schema: Some(serde_json::json!({ "type": "object" })),
                ..WorkflowAgentOpts::default()
            },
        )
        .await
        .expect_err("schema run must require StructuredOutput");
    assert!(err.contains("subagent completed without calling StructuredOutput"));
}

#[tokio::test]
async fn run_agent_retries_after_stall_then_succeeds() {
    use std::sync::atomic::AtomicI64;
    use std::sync::atomic::Ordering;

    use coco_tool_runtime::AgentHandle;
    use coco_tool_runtime::AgentSpawnRequest;
    use coco_tool_runtime::AgentSpawnResponse;
    use coco_tool_runtime::AgentSpawnStatus;
    use coco_tool_runtime::TeamMessageDispatchResult;
    use coco_workflow_runtime::WorkflowHost;

    // The first spawn attempt hangs past the (tiny) stall window so the
    // watchdog aborts + retries; the second attempt returns immediately. The
    // resolved model on the response proves the retry attempt is the one whose
    // result we surface.
    #[derive(Debug)]
    struct StallingHandle {
        calls: AtomicI64,
    }

    #[async_trait::async_trait]
    impl AgentHandle for StallingHandle {
        async fn spawn_agent(
            &self,
            _request: AgentSpawnRequest,
        ) -> Result<AgentSpawnResponse, String> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 1 {
                // Hang well past the stall window; the watchdog aborts this.
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
            Ok(AgentSpawnResponse {
                status: AgentSpawnStatus::Completed,
                result: Some("done".to_string()),
                model: Some("anthropic/resolved-model".to_string()),
                ..Default::default()
            })
        }
        async fn send_message(
            &self,
            _to: &str,
            _content: &str,
            _summary: Option<&str>,
        ) -> Result<TeamMessageDispatchResult, String> {
            Err("unused".into())
        }
        async fn query_agent_status(&self, _agent_id: &str) -> Result<AgentSpawnResponse, String> {
            Err("unused".into())
        }
        async fn get_agent_output(&self, _agent_id: &str) -> Result<String, String> {
            Err("unused".into())
        }
    }

    let handle = Arc::new(StallingHandle {
        calls: AtomicI64::new(0),
    });
    let mut host = host();
    host.agent = handle.clone();
    let result = host
        .run_agent(
            "compute".to_string(),
            WorkflowAgentOpts {
                stall_ms: Some(20),
                ..WorkflowAgentOpts::default()
            },
        )
        .await
        .expect("run_agent retries to success");

    // Two attempts ran (the first stalled), and the second's result surfaced.
    assert_eq!(handle.calls.load(Ordering::SeqCst), 2);
    assert_eq!(result.value, serde_json::json!("done"));
    assert_eq!(result.model.as_deref(), Some("anthropic/resolved-model"));
}

#[tokio::test]
async fn run_agent_fails_after_exhausting_stall_retries() {
    use coco_tool_runtime::AgentHandle;
    use coco_tool_runtime::AgentSpawnRequest;
    use coco_tool_runtime::AgentSpawnResponse;
    use coco_tool_runtime::TeamMessageDispatchResult;
    use coco_workflow_runtime::WORKFLOW_STALL_RETRY;
    use coco_workflow_runtime::WorkflowHost;

    // Every attempt hangs → the watchdog exhausts its retries and returns Err
    // (which the engine maps to a rejected promise → null in parallel/pipeline).
    #[derive(Debug)]
    struct AlwaysStallHandle;

    #[async_trait::async_trait]
    impl AgentHandle for AlwaysStallHandle {
        async fn spawn_agent(
            &self,
            _request: AgentSpawnRequest,
        ) -> Result<AgentSpawnResponse, String> {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            unreachable!("aborted by the watchdog before completing")
        }
        async fn send_message(
            &self,
            _to: &str,
            _content: &str,
            _summary: Option<&str>,
        ) -> Result<TeamMessageDispatchResult, String> {
            Err("unused".into())
        }
        async fn query_agent_status(&self, _agent_id: &str) -> Result<AgentSpawnResponse, String> {
            Err("unused".into())
        }
        async fn get_agent_output(&self, _agent_id: &str) -> Result<String, String> {
            Err("unused".into())
        }
    }

    let mut host = host();
    host.agent = Arc::new(AlwaysStallHandle);
    let err = host
        .run_agent(
            "compute".to_string(),
            WorkflowAgentOpts {
                stall_ms: Some(10),
                ..WorkflowAgentOpts::default()
            },
        )
        .await
        .expect_err("all attempts stall");
    assert!(err.contains("stalled"));
    assert!(err.contains(&WORKFLOW_STALL_RETRY.to_string()));
}

#[tokio::test]
async fn run_agent_surfaces_resolved_model() {
    use coco_tool_runtime::AgentHandle;
    use coco_tool_runtime::AgentSpawnRequest;
    use coco_tool_runtime::AgentSpawnResponse;
    use coco_tool_runtime::AgentSpawnStatus;
    use coco_tool_runtime::TeamMessageDispatchResult;
    use coco_workflow_runtime::WorkflowHost;

    // A completed spawn that reports its resolved model must thread that model
    // onto the WorkflowAgentResult (observability).
    #[derive(Debug)]
    struct ModelHandle;

    #[async_trait::async_trait]
    impl AgentHandle for ModelHandle {
        async fn spawn_agent(
            &self,
            _request: AgentSpawnRequest,
        ) -> Result<AgentSpawnResponse, String> {
            Ok(AgentSpawnResponse {
                status: AgentSpawnStatus::Completed,
                result: Some("ok".to_string()),
                model: Some("anthropic/opus".to_string()),
                ..Default::default()
            })
        }
        async fn send_message(
            &self,
            _to: &str,
            _content: &str,
            _summary: Option<&str>,
        ) -> Result<TeamMessageDispatchResult, String> {
            Err("unused".into())
        }
        async fn query_agent_status(&self, _agent_id: &str) -> Result<AgentSpawnResponse, String> {
            Err("unused".into())
        }
        async fn get_agent_output(&self, _agent_id: &str) -> Result<String, String> {
            Err("unused".into())
        }
    }

    let mut host = host();
    host.agent = Arc::new(ModelHandle);
    let result = host
        .run_agent("compute".to_string(), WorkflowAgentOpts::default())
        .await
        .expect("run_agent ok");
    assert_eq!(result.model.as_deref(), Some("anthropic/opus"));
}

#[tokio::test]
async fn build_request_rejects_remote_isolation() {
    let host = host();
    let err = host
        .build_request(
            "research".to_string(),
            &WorkflowAgentOpts {
                isolation: Some(coco_types::AgentIsolation::Remote),
                ..WorkflowAgentOpts::default()
            },
            test_abort(),
        )
        .expect_err("remote is unavailable");

    assert!(err.contains("remote"));
}

/// Build the host through `Arc::new_cyclic` (so the `me` self-ref is live, which
/// `run_nested_workflow` needs to re-enter the engine) with `cwd` pointed at a
/// temp dir for nested source resolution.
fn cyclic_host_with_cwd(cwd: std::path::PathBuf) -> Arc<WorkflowRunHost> {
    Arc::new_cyclic(|me| WorkflowRunHost {
        agent: Arc::new(NoOpAgentHandle),
        task_handle: Arc::new(NoOpBackgroundTaskHandle),
        task_id: "wtest".to_string(),
        main_handle: tokio::runtime::Handle::current(),
        spawn_ctx: WorkflowSpawnContext {
            session_id: Some(coco_types::SessionId::try_new("session").unwrap()),
            invoking_agent_id: None,
            tool_use_id: None,
            features: Arc::new(coco_types::Features::with_defaults()),
            skill_overrides: Arc::new(coco_config::SkillOverrideTiers::default()),
            tool_overrides: Arc::new(coco_types::ToolOverrides::none()),
            parent_tool_filter: coco_types::ToolFilter::unrestricted(),
            active_shell_tool: coco_types::ActiveShellTool::Disabled,
            log_assistant_responses: None,
            parent_mode: coco_types::PermissionMode::Default,
            agent_catalog: None,
            total_token_budget: None,
            workflow_abort: coco_tool_runtime::TurnAbortSignal::from_token(
                tokio_util::sync::CancellationToken::new(),
            ),
            cwd: Some(cwd),
        },
        budget_spent_tokens: std::sync::atomic::AtomicI64::new(0),
        semaphore: Arc::new(tokio::sync::Semaphore::new(
            super::workflow_local_concurrency(),
        )),
        journal: Arc::new(super::WorkflowJournal::new(None)),
        me: me.clone() as std::sync::Weak<dyn coco_workflow_runtime::WorkflowHost>,
    })
}

/// Drive an async closure on a current-thread runtime inside a `LocalSet` (the
/// engine the host re-enters is `!Send`).
fn block_on_local<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    let runtime = super::LocalWorkflowRuntime::new().expect("local workflow runtime");
    runtime.block_on(future)
}

#[test]
fn run_nested_workflow_resolves_named_child_and_returns_value() {
    use coco_workflow_runtime::WorkflowHost;

    let dir = tempfile::tempdir().expect("tempdir");
    let workflows = dir.path().join(".cocode").join("workflows");
    std::fs::create_dir_all(&workflows).expect("mkdir");
    // The saved workflow is invoked by its parsed meta.name ("Child Build"),
    // not the file stem. The body returns a constant so no real subagent runs.
    std::fs::write(
        workflows.join("child-build.ts"),
        r#"export const meta = { name: "Child Build", description: "x" };
           return { ok: true, got: args.k };"#,
    )
    .expect("write");

    let cwd = dir.path().to_path_buf();
    let result = block_on_local(async move {
        let host = cyclic_host_with_cwd(cwd);
        host.run_nested_workflow(
            "Child Build".to_string(),
            serde_json::json!({ "k": 7 }),
            /*depth*/ 1,
        )
        .await
    })
    .expect("nested run ok");
    assert_eq!(result, serde_json::json!({ "ok": true, "got": 7 }));
}

#[test]
fn run_nested_workflow_resolves_script_path_ref() {
    use coco_workflow_runtime::WorkflowHost;

    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("inline-child.ts");
    std::fs::write(
        &script,
        r#"export const meta = { name: "inline", description: "x" };
           return "from-script-path";"#,
    )
    .expect("write");

    let cwd = dir.path().to_path_buf();
    let path_ref = script.to_string_lossy().to_string();
    let result = block_on_local(async move {
        let host = cyclic_host_with_cwd(cwd);
        host.run_nested_workflow(path_ref, serde_json::Value::Null, /*depth*/ 1)
            .await
    })
    .expect("nested run ok");
    assert_eq!(result, serde_json::json!("from-script-path"));
}

#[test]
fn run_nested_workflow_unknown_name_rejects() {
    use coco_workflow_runtime::WorkflowHost;

    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path().to_path_buf();
    let err = block_on_local(async move {
        let host = cyclic_host_with_cwd(cwd);
        host.run_nested_workflow("does-not-exist".to_string(), serde_json::Value::Null, 1)
            .await
    })
    .expect_err("unknown name rejects");
    assert!(err.contains("was not launched"), "got: {err}");
}

#[test]
fn run_nested_workflow_child_workflow_call_is_guarded() {
    use coco_workflow_runtime::WorkflowHost;

    // The child (run at depth 1) calls workflow() itself — the engine installs a
    // throwing workflow() at depth >= 1, so the child catches the one-level error.
    let dir = tempfile::tempdir().expect("tempdir");
    let workflows = dir.path().join(".cocode").join("workflows");
    std::fs::create_dir_all(&workflows).expect("mkdir");
    std::fs::write(
        workflows.join("nester.ts"),
        r#"export const meta = { name: "nester", description: "x" };
           try { await workflow("whatever"); return "unreachable"; }
           catch (e) { return String(e.message || e); }"#,
    )
    .expect("write");

    let cwd = dir.path().to_path_buf();
    let result = block_on_local(async move {
        let host = cyclic_host_with_cwd(cwd);
        host.run_nested_workflow("nester".to_string(), serde_json::Value::Null, 1)
            .await
    })
    .expect("nested run ok");
    assert_eq!(
        result,
        serde_json::json!(coco_workflow_runtime::WORKFLOW_NESTING_LIMIT_ERROR)
    );
}
