//! Bridges the workflow engine's `WorkflowHost` callbacks to the live subagent
//! system (`AgentHandle`) and the task progress channel.
//!
//! The engine is `!Send` (rquickjs `Ctx`/`Value`), so it runs on a dedicated OS
//! thread with a current-thread runtime + `LocalSet`. `agent()` and progress
//! bridge back to the main multi-thread runtime via its `Handle`: subagent
//! spawns run on the main runtime (where the agent system lives) and the
//! dedicated thread awaits their `JoinHandle`.

use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use coco_tool_runtime::AgentCompletionPayload;
use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::AgentSpawnRequest;
use coco_tool_runtime::AgentSpawnStatus;
use coco_tool_runtime::SpawnMode;
use coco_tool_runtime::TaskHandleRef;
use coco_types::WorkflowProgressEvent;
use coco_workflow_runtime::WORKFLOW_VM_TIMEOUT;
use coco_workflow_runtime::WorkflowAgentOpts;
use coco_workflow_runtime::WorkflowAgentResult;
use coco_workflow_runtime::WorkflowEngine;
use coco_workflow_runtime::WorkflowHost;
use tokio_util::sync::CancellationToken;

/// Parent-context fields captured at launch, needed to build faithful subagent
/// spawn requests (inheritance must thread through; subagents narrow, never
/// widen).
pub(crate) struct WorkflowSpawnContext {
    pub session_id: String,
    pub invoking_agent_id: Option<String>,
    pub tool_use_id: Option<String>,
    pub features: Arc<coco_types::Features>,
    pub skill_overrides: Arc<coco_config::SkillOverrideTiers>,
    pub tool_overrides: Arc<coco_types::ToolOverrides>,
    pub parent_tool_filter: coco_types::ToolFilter,
    pub active_shell_tool: coco_types::ActiveShellTool,
    pub parent_mode: coco_types::PermissionMode,
    pub agent_catalog: Option<Arc<coco_subagent::AgentCatalogSnapshot>>,
    pub total_token_budget: Option<i64>,
    pub workflow_abort: coco_tool_runtime::TurnAbortSignal,
}

struct WorkflowRunHost {
    agent: AgentHandleRef,
    task_handle: TaskHandleRef,
    task_id: String,
    main_handle: tokio::runtime::Handle,
    spawn_ctx: WorkflowSpawnContext,
    budget_spent_tokens: AtomicI64,
}

impl WorkflowRunHost {
    fn build_request(
        &self,
        prompt: String,
        opts: &WorkflowAgentOpts,
    ) -> Result<AgentSpawnRequest, String> {
        let ctx = &self.spawn_ctx;
        if opts.isolation == Some(coco_types::AgentIsolation::Remote) {
            return Err("Isolation 'remote' is not available in this build.".to_string());
        }
        let definition = Some(self.definition_for_opts(opts)?);
        let isolation = opts
            .isolation
            .or_else(|| definition.as_ref().map(|def| def.isolation))
            .filter(|isolation| *isolation != coco_types::AgentIsolation::None);
        Ok(AgentSpawnRequest {
            prompt,
            description: Some(
                opts.label
                    .clone()
                    .unwrap_or_else(|| "workflow step".to_string()),
            ),
            subagent_type: opts.agent_type.clone(),
            // Foreground: we await the result inline. The universal
            // subagent deny-list already blocks Agent + Workflow, so a
            // workflow subagent cannot recurse.
            run_in_background: false,
            session_id: ctx.session_id.clone(),
            mode: Some(coco_permissions::resolve_subagent_mode(
                ctx.parent_mode,
                None,
            )),
            features: Some(ctx.features.clone()),
            skill_overrides: Some(ctx.skill_overrides.clone()),
            tool_overrides: Some(ctx.tool_overrides.clone()),
            parent_tool_filter: Some(ctx.parent_tool_filter.clone()),
            active_shell_tool: ctx.active_shell_tool,
            spawn_mode: SpawnMode::Fresh,
            tool_use_id: ctx.tool_use_id.clone(),
            invoking_agent_id: ctx.invoking_agent_id.clone(),
            isolation,
            definition,
            is_non_interactive: true,
            parent_turn_abort: Some(ctx.workflow_abort.clone()),
            ..Default::default()
        })
    }

    fn definition_for_opts(
        &self,
        opts: &WorkflowAgentOpts,
    ) -> Result<Arc<coco_types::AgentDefinition>, String> {
        let agent_name = opts
            .agent_type
            .as_deref()
            .unwrap_or(coco_types::SubagentType::GeneralPurpose.as_str());
        let mut definition = self
            .spawn_ctx
            .agent_catalog
            .as_ref()
            .and_then(|catalog| catalog.find_active(agent_name).cloned())
            .unwrap_or_else(|| coco_types::AgentDefinition {
                agent_type: agent_name
                    .parse()
                    .expect("AgentTypeId::from_str is Infallible"),
                name: agent_name.to_string(),
                source: coco_types::AgentSource::BuiltIn,
                ..Default::default()
            });

        if let Some(model) = opts.model.as_ref().filter(|model| !model.trim().is_empty()) {
            definition.model = Some(model.trim().to_string());
        }
        if let Some(effort) = opts
            .effort
            .as_deref()
            .filter(|effort| !effort.trim().is_empty())
        {
            definition.effort = Some(effort.trim().parse::<coco_types::ReasoningEffort>()?);
        }
        if let Some(isolation) = opts.isolation {
            definition.isolation = isolation;
        }
        Ok(Arc::new(definition))
    }
}

#[async_trait::async_trait]
impl WorkflowHost for WorkflowRunHost {
    async fn run_agent(
        &self,
        prompt: String,
        opts: WorkflowAgentOpts,
    ) -> Result<WorkflowAgentResult, String> {
        let request = self.build_request(prompt, &opts)?;
        let agent = self.agent.clone();
        // Spawn on the main runtime (the agent system runs there); await the
        // result from this dedicated engine thread.
        let response = self
            .main_handle
            .spawn(async move { agent.spawn_agent(request).await })
            .await
            .map_err(|e| format!("workflow subagent task join error: {e}"))??;

        match response.status {
            AgentSpawnStatus::Completed => {
                let text = response.result.unwrap_or_default();
                // With a schema the subagent emits JSON as its final text;
                // parse it, else return the raw text as a string value.
                let value = if opts.schema.is_some() {
                    serde_json::from_str::<serde_json::Value>(&text)
                        .unwrap_or(serde_json::Value::String(text))
                } else {
                    serde_json::Value::String(text)
                };
                let tokens = response.input_tokens + response.output_tokens;
                Ok(WorkflowAgentResult {
                    value,
                    model: None,
                    tokens: Some(tokens),
                    tool_calls: i32::try_from(response.total_tool_use_count).ok(),
                    duration_ms: Some(response.duration_ms),
                })
            }
            AgentSpawnStatus::Failed => Err(response
                .error
                .unwrap_or_else(|| "workflow subagent failed".to_string())),
            other => Err(format!(
                "workflow subagent returned unexpected status {other:?}"
            )),
        }
    }

    fn push_progress(&self, event: WorkflowProgressEvent) {
        let task_handle = self.task_handle.clone();
        let task_id = self.task_id.clone();
        // Fire-and-forget onto the main runtime so `log()`/`phase()` stay sync.
        self.main_handle.spawn(async move {
            task_handle.push_workflow_progress(&task_id, event).await;
        });
    }

    fn budget_total_tokens(&self) -> Option<i64> {
        self.spawn_ctx.total_token_budget
    }

    fn budget_spent_tokens(&self) -> i64 {
        self.budget_spent_tokens.load(Ordering::Relaxed)
    }

    fn record_agent_tokens(&self, tokens: i64) {
        self.budget_spent_tokens
            .fetch_add(tokens, Ordering::Relaxed);
    }
}

/// Launch the workflow engine on a dedicated OS thread (the engine is `!Send`).
/// Fire-and-forget: returns immediately; the thread runs the script to
/// completion, then marks the task terminal. `agent()`/progress bridge to
/// `main_handle`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_workflow_engine(
    script: String,
    args: serde_json::Value,
    agent: AgentHandleRef,
    task_handle: TaskHandleRef,
    task_id: String,
    cancel: CancellationToken,
    spawn_ctx: WorkflowSpawnContext,
    main_handle: tokio::runtime::Handle,
) {
    let thread = std::thread::Builder::new()
        .name(format!("workflow-{task_id}"))
        .spawn(move || {
            let host: Arc<dyn WorkflowHost> = Arc::new(WorkflowRunHost {
                agent,
                task_handle: task_handle.clone(),
                task_id: task_id.clone(),
                main_handle,
                spawn_ctx,
                budget_spent_tokens: AtomicI64::new(0),
            });
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!(target: "coco::workflow", %error, "failed to build workflow runtime");
                    return;
                }
            };
            let local = tokio::task::LocalSet::new();
            local.block_on(&runtime, async move {
                let outcome =
                    WorkflowEngine::run(script, args, host, cancel, WORKFLOW_VM_TIMEOUT).await;
                match outcome {
                    Ok(value) => {
                        task_handle
                            .mark_completed(
                                &task_id,
                                AgentCompletionPayload {
                                    result: Some(render_result(&value)),
                                    usage: None,
                                    worktree: None,
                                },
                            )
                            .await;
                    }
                    Err(error) => {
                        task_handle.mark_failed(&task_id, &error.to_string()).await;
                    }
                }
            });
        });
    if let Err(error) = thread {
        tracing::error!(target: "coco::workflow", %error, "failed to spawn workflow engine thread");
    }
}

fn render_result(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => "Workflow completed.".to_string(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

#[cfg(test)]
#[path = "workflow_host.test.rs"]
mod tests;
