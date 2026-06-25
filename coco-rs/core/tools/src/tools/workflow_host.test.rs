use std::sync::Arc;

use coco_tool_runtime::NoOpAgentHandle;
use coco_tool_runtime::NoOpBackgroundTaskHandle;
use coco_workflow_runtime::WorkflowAgentOpts;

use super::WorkflowRunHost;
use super::WorkflowSpawnContext;

fn host() -> WorkflowRunHost {
    WorkflowRunHost {
        agent: Arc::new(NoOpAgentHandle),
        task_handle: Arc::new(NoOpBackgroundTaskHandle),
        task_id: "wtest".to_string(),
        main_handle: tokio::runtime::Handle::current(),
        spawn_ctx: WorkflowSpawnContext {
            session_id: "session".to_string(),
            invoking_agent_id: Some("parent-agent".to_string()),
            tool_use_id: Some("toolu_1".to_string()),
            features: Arc::new(coco_types::Features::with_defaults()),
            skill_overrides: Arc::new(coco_config::SkillOverrideTiers::default()),
            tool_overrides: Arc::new(coco_types::ToolOverrides::none()),
            parent_tool_filter: coco_types::ToolFilter::unrestricted(),
            active_shell_tool: coco_types::ActiveShellTool::Disabled,
            parent_mode: coco_types::PermissionMode::Default,
            agent_catalog: None,
            total_token_budget: Some(100),
            workflow_abort: coco_tool_runtime::TurnAbortSignal::from_token(
                tokio_util::sync::CancellationToken::new(),
            ),
        },
        budget_spent_tokens: std::sync::atomic::AtomicI64::new(0),
        semaphore: Arc::new(tokio::sync::Semaphore::new(
            super::workflow_local_concurrency(),
        )),
    }
}

#[test]
fn workflow_local_concurrency_within_floor_and_ceiling() {
    let width = super::workflow_local_concurrency();
    assert!(width >= super::WORKFLOW_CONCURRENCY_FLOOR);
    assert!(width <= super::WORKFLOW_CONCURRENCY_CEILING);
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
        )
        .expect("request");

    assert_eq!(request.subagent_type.as_deref(), Some("Explore"));
    assert_eq!(
        request.isolation,
        Some(coco_types::AgentIsolation::Worktree)
    );
    let definition = request.definition.expect("synthetic definition");
    assert_eq!(definition.name, "Explore");
    assert_eq!(definition.model.as_deref(), Some("anthropic/custom-model"));
    assert_eq!(definition.effort, Some(coco_types::ReasoningEffort::High));
    assert_eq!(definition.isolation, coco_types::AgentIsolation::Worktree);
    assert!(request.parent_turn_abort.is_some());
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
        )
        .expect("request");
    let carried = request.output_schema.expect("output_schema is Some");
    assert_eq!(*carried.as_ref(), schema);
}

#[tokio::test]
async fn build_request_omits_output_schema_when_absent() {
    let host = host();
    let request = host
        .build_request("research".to_string(), &WorkflowAgentOpts::default())
        .expect("request");
    assert!(request.output_schema.is_none());
}

#[tokio::test]
async fn run_agent_uses_captured_structured_output() {
    use coco_tool_runtime::AgentHandle;
    use coco_tool_runtime::AgentSpawnRequest;
    use coco_tool_runtime::AgentSpawnResponse;
    use coco_tool_runtime::AgentSpawnStatus;
    use coco_tool_runtime::CreateTeamRequest;
    use coco_tool_runtime::CreateTeamResult;
    use coco_tool_runtime::DeleteTeamResult;
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
        async fn create_team(
            &self,
            _request: CreateTeamRequest,
        ) -> Result<CreateTeamResult, String> {
            Err("unused".into())
        }
        async fn delete_team(&self) -> Result<DeleteTeamResult, String> {
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
async fn run_agent_falls_back_to_text_parse_without_capture() {
    use coco_tool_runtime::AgentHandle;
    use coco_tool_runtime::AgentSpawnRequest;
    use coco_tool_runtime::AgentSpawnResponse;
    use coco_tool_runtime::AgentSpawnStatus;
    use coco_tool_runtime::CreateTeamRequest;
    use coco_tool_runtime::CreateTeamResult;
    use coco_tool_runtime::DeleteTeamResult;
    use coco_tool_runtime::TeamMessageDispatchResult;
    use coco_workflow_runtime::WorkflowHost;

    // No captured structured value → host falls back to parsing the final
    // text as JSON (the last-resort path; behaviour never worse than before).
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
        async fn create_team(
            &self,
            _request: CreateTeamRequest,
        ) -> Result<CreateTeamResult, String> {
            Err("unused".into())
        }
        async fn delete_team(&self) -> Result<DeleteTeamResult, String> {
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
    assert_eq!(result.value, serde_json::json!({ "answer": "parsed" }));
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
        )
        .expect_err("remote is unavailable");

    assert!(err.contains("remote"));
}
