//! Real child-engine subagent spawn — the parent model calls the `Agent`
//! tool, a **real child `QueryEngine`** runs against the shared scripted
//! model, the child executes a tool with an observable side effect, and
//! its output folds back into the parent's `Agent` tool result.
//!
//! Why this matters
//! ----------------
//! The offline harnesses default to `NoOpAgentHandle`, whose
//! `spawn_agent` returns an error — so prior agent-team scenarios could
//! only assert "the tool was callable", never "a subagent actually ran".
//! This test installs a `TestAgentHandle` backed by the production
//! `coco_query::agent_adapter::QueryEngineAdapter`: spawn runs a genuine
//! child engine (same code path the SDK/coordinator drive) and folds the
//! result back, mirroring codex's `agent_execution` / `subagent_notifications`.
//!
//! Determinism: parent and child share ONE `ScriptedModel` FIFO queue, so
//! replies are consumed in a fixed order:
//!   1. parent  → `Agent` tool call
//!   2. child   → `Write` tool call (observable side effect)
//!   3. child   → final text (becomes the subagent's result)
//!   4. parent  → final text

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use async_trait::async_trait;
use coco_inference::LanguageModel;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_query::agent_adapter::QueryEngineAdapter;
use coco_query::agent_adapter::QueryEngineFactory;
use coco_tool_runtime::AgentHandle;
use coco_tool_runtime::AgentQueryConfig;
use coco_tool_runtime::AgentQueryEngine;
use coco_tool_runtime::AgentRunIdentity;
use coco_tool_runtime::AgentRunKind;
use coco_tool_runtime::AgentSpawnRequest;
use coco_tool_runtime::AgentSpawnResponse;
use coco_tool_runtime::AgentSpawnStatus;
use coco_tool_runtime::CreateTeamRequest;
use coco_tool_runtime::CreateTeamResult;
use coco_tool_runtime::ToolRegistry;
use coco_types::LlmModelSelection;
use coco_types::PermissionMode;
use tokio_util::sync::CancellationToken;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

/// A minimal model-spawnable [`AgentHandle`] that runs each spawn as a
/// real child `QueryEngine` via the production [`QueryEngineAdapter`].
/// Only `spawn_agent` is meaningful; the team / messaging methods are
/// unused by this scenario and fail loudly if exercised.
struct TestAgentHandle {
    adapter: QueryEngineAdapter,
}

#[async_trait]
impl AgentHandle for TestAgentHandle {
    async fn spawn_agent(&self, request: AgentSpawnRequest) -> Result<AgentSpawnResponse, String> {
        // Build a child run config. Default fills the ~40 fields the
        // adapter reads; we override only what the child needs to run a
        // tool offline: a fresh identity, an explicit bypass so the
        // child's Write executes without a permission bridge, and a turn
        // cap so an underspecified queue can't spin.
        let config = AgentQueryConfig {
            system_prompt: "You are a deterministic test subagent.".to_string(),
            identity: AgentRunIdentity {
                session_id: "test-session".to_string(),
                agent_id: "child-agent-1".to_string(),
                kind: AgentRunKind::Subagent,
            },
            permission_mode: PermissionMode::BypassPermissions,
            bypass_permissions_available: true,
            max_turns: Some(8),
            ..AgentQueryConfig::default()
        };

        let result = self
            .adapter
            .execute_query(&request.prompt, config)
            .await
            .map_err(|e| e.to_string())?;

        Ok(AgentSpawnResponse {
            status: AgentSpawnStatus::Completed,
            agent_id: Some("child-agent-1".to_string()),
            result: result.response_text,
            total_tool_use_count: result.tool_use_count,
            total_tokens: result.input_tokens + result.output_tokens,
            input_tokens: result.input_tokens,
            output_tokens: result.output_tokens,
            prompt: Some(request.prompt),
            ..Default::default()
        })
    }

    async fn send_message(&self, _to: &str, _content: &str) -> Result<String, String> {
        Err("send_message unused in TestAgentHandle".to_string())
    }

    async fn create_team(&self, _request: CreateTeamRequest) -> Result<CreateTeamResult, String> {
        Err("create_team unused in TestAgentHandle".to_string())
    }

    async fn delete_team(&self) -> Result<String, String> {
        Err("delete_team unused in TestAgentHandle".to_string())
    }

    async fn query_agent_status(&self, _agent_id: &str) -> Result<AgentSpawnResponse, String> {
        Err("query_agent_status unused in TestAgentHandle".to_string())
    }

    async fn get_agent_output(&self, _agent_id: &str) -> Result<String, String> {
        Err("get_agent_output unused in TestAgentHandle".to_string())
    }
}

const CHILD_MARKER: &str = "CHILD-RAN";
const CHILD_RESULT_MARKER: &str = "CHILD-DID-THE-WORK";

pub async fn run() -> Result<()> {
    // Pre-mint the workdir so we can bake the child's absolute output
    // path into a scripted `Write` call before the harness boots.
    let workdir = crate::common::tmpdir::make("coco-tests-agentteam-")?;
    let child_file = workdir.path().join("child-output.txt");
    let child_file_str = child_file.to_string_lossy().to_string();

    let mut harness = TuiHarness::builder()
        .with_workdir(workdir)
        .with_agent_handle_factory(|model: Arc<dyn LanguageModel>, tools: Arc<ToolRegistry>| {
            // The factory builds each child engine from the SAME scripted
            // model + tool registry the parent uses, so the child runs
            // deterministically off the shared reply queue.
            let factory: QueryEngineFactory = Arc::new(
                move |cfg: QueryEngineConfig,
                      _sel: LlmModelSelection,
                      cancel: Option<CancellationToken>|
                      -> Pin<Box<dyn Future<Output = QueryEngine> + Send>> {
                    let model = model.clone();
                    let tools = tools.clone();
                    Box::pin(async move {
                        let runtimes = coco_query::test_support::model_runtime_registry(model);
                        QueryEngine::new(cfg, runtimes, tools, cancel.unwrap_or_default(), None)
                    })
                },
            );
            Arc::new(TestAgentHandle {
                adapter: QueryEngineAdapter::new(factory),
            }) as Arc<dyn AgentHandle>
        })
        .with_replies([
            // 1. Parent spawns a subagent.
            Reply::tool_call(
                "call-agent",
                "Agent",
                serde_json::json!({
                    "subagent_type": "general-purpose",
                    "description": "write report",
                    "prompt": "Write the report file, then report completion.",
                }),
            ),
            // 2. Child writes a file — the observable proof a real engine ran.
            Reply::tool_call(
                "child-write",
                "Write",
                serde_json::json!({ "file_path": child_file_str, "content": CHILD_MARKER }),
            ),
            // 3. Child's final text → becomes the subagent's `result`.
            Reply::text(format!("{CHILD_RESULT_MARKER}: report written.")),
            // 4. Parent's final text, referencing the folded-back result.
            Reply::text("Parent done; the subagent reported success."),
        ])
        .build()
        .await?;

    harness.submit("delegate the report to a subagent").await;
    let clean = harness.pump_until_idle(Duration::from_secs(10)).await?;

    // (1) The CHILD engine actually ran AND executed a tool with a real
    // side effect. `NoOpAgentHandle` could never produce this file — its
    // spawn returns an error before any child runs.
    let written = std::fs::read_to_string(&child_file).map_err(|e| {
        anyhow!("child output file missing — the child engine never ran a tool: {e}")
    })?;
    assert!(
        written.contains(CHILD_MARKER),
        "agent_team_spawn: child Write produced unexpected content: {written:?}",
    );

    // (2) The child's result folded back into the parent's `Agent` tool
    // result — the model-visible return value of the spawn.
    let (agent_result, is_error) = harness
        .find_tool_result("Agent")
        .ok_or_else(|| anyhow!("parent transcript should carry an `Agent` tool result"))?;
    assert!(
        !is_error,
        "agent_team_spawn: Agent tool result should be success, got error: {agent_result}",
    );
    assert!(
        agent_result.contains(CHILD_RESULT_MARKER),
        "agent_team_spawn: parent's Agent tool result should contain the child's output \
         ({CHILD_RESULT_MARKER}), got: {agent_result}",
    );

    // (3) The whole session terminated cleanly (spawn folded in mid-loop,
    // not via an error path).
    assert!(
        clean,
        "agent_team_spawn: expected a clean SessionResult after the subagent folded back",
    );

    harness.shutdown().await;
    Ok(())
}
