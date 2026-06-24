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
    }
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
