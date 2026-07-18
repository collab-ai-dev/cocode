//! Bridges the workflow engine's `WorkflowHost` callbacks to the live subagent
//! system (`AgentHandle`) and the task progress channel.
//!
//! The engine is `!Send` (rquickjs `Ctx`/`Value`), so it runs on a dedicated OS
//! thread with a current-thread runtime + `LocalSet`. `agent()` and progress
//! bridge back to the main multi-thread runtime via its `Handle`: subagent
//! spawns run on the main runtime (where the agent system lives) and the
//! dedicated thread awaits their `JoinHandle`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use tokio::sync::Semaphore;

use coco_tool_runtime::AgentCompletionPayload;
use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::AgentSpawnExecution;
use coco_tool_runtime::AgentSpawnInheritance;
use coco_tool_runtime::AgentSpawnInput;
use coco_tool_runtime::AgentSpawnPermissions;
use coco_tool_runtime::AgentSpawnRequest;
use coco_tool_runtime::AgentSpawnRouting;
use coco_tool_runtime::AgentSpawnStatus;
use coco_tool_runtime::AgentSpawnTelemetry;
use coco_tool_runtime::SpawnMode;
use coco_tool_runtime::TaskHandleRef;
use coco_types::SessionId;
use coco_types::WorkflowProgressEvent;
use coco_workflow_runtime::AgentCacheKey;
use coco_workflow_runtime::WORKFLOW_STALL_MS_DEFAULT;
use coco_workflow_runtime::WORKFLOW_STALL_RETRY;
use coco_workflow_runtime::WORKFLOW_SYNC_EVAL_BUDGET;
use coco_workflow_runtime::WorkflowAgentOpts;
use coco_workflow_runtime::WorkflowAgentResult;
use coco_workflow_runtime::WorkflowEngine;
use coco_workflow_runtime::WorkflowHost;
use tokio_util::sync::CancellationToken;

use super::workflow_journal::WorkflowJournal;

/// Parent-context fields captured at launch, needed to build faithful subagent
/// spawn requests (inheritance must thread through; subagents narrow, never
/// widen).
pub(crate) struct WorkflowSpawnContext {
    pub session_id: Option<SessionId>,
    pub invoking_agent_id: Option<String>,
    pub tool_use_id: Option<String>,
    pub features: Arc<coco_types::Features>,
    pub skill_overrides: Arc<coco_config::SkillOverrideTiers>,
    pub tool_overrides: Arc<coco_types::ToolOverrides>,
    pub parent_tool_filter: coco_types::ToolFilter,
    pub active_shell_tool: coco_types::ActiveShellTool,
    pub log_assistant_responses: Option<bool>,
    pub parent_mode: coco_types::PermissionMode,
    pub mcp_tool_exposure: coco_types::McpToolExposure,
    pub mcp_server_tool_exposure: std::collections::HashMap<String, coco_types::McpToolExposure>,
    pub agent_catalog: Option<Arc<coco_subagent::AgentCatalogSnapshot>>,
    pub total_token_budget: Option<i64>,
    pub workflow_abort: coco_tool_runtime::TurnAbortSignal,
    /// Working directory used to resolve nested `workflow(nameOrRef)` sources
    /// (saved-workflow name lookup + relative `{scriptPath}` resolution). `None`
    /// falls back to the process cwd inside `resolve_workflow_source`.
    pub cwd: Option<PathBuf>,
}

/// Ceiling on the local workflow executor width (CC `min(16, …)`).
const WORKFLOW_CONCURRENCY_CEILING: usize = 16;
/// Floor on the local workflow executor width (CC `max(2, …)`).
const WORKFLOW_CONCURRENCY_FLOOR: usize = 2;
/// Cores held back as headroom when sizing the executor (CC `cpus - 2`).
const WORKFLOW_CONCURRENCY_HEADROOM: usize = 2;

/// Local workflow concurrency width: `min(16, max(2, cpus - 2))` (CC parity).
/// A FIFO counting semaphore of this width admits each `agent()` dispatch, so
/// `parallel()`/`pipeline()` still fire every thunk but only this many run at
/// once.
fn workflow_local_concurrency() -> usize {
    let available = std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(4)
        .saturating_sub(WORKFLOW_CONCURRENCY_HEADROOM);
    // FLOOR <= CEILING by construction, so clamp cannot panic.
    available.clamp(WORKFLOW_CONCURRENCY_FLOOR, WORKFLOW_CONCURRENCY_CEILING)
}

struct WorkflowRunHost {
    agent: AgentHandleRef,
    task_handle: TaskHandleRef,
    task_id: String,
    main_handle: tokio::runtime::Handle,
    spawn_ctx: WorkflowSpawnContext,
    budget_spent_tokens: AtomicI64,
    /// FIFO counting semaphore bounding concurrent subagent spawns.
    semaphore: Arc<Semaphore>,
    /// Resume cache + append-only journal. On a fresh run it starts empty and
    /// records each result; on resume it is hydrated from the prior journal so
    /// completed `agent()` results replay without re-spawning.
    journal: Arc<WorkflowJournal>,
    /// Weak self-reference so `run_nested_workflow` can re-enter
    /// [`WorkflowEngine::run`] with the SAME `Arc<dyn WorkflowHost>` — that
    /// shared host is exactly what shares the parent's semaphore, token budget,
    /// journal, abort signal, and agent counter with the child workflow. Set via
    /// `Arc::new_cyclic` at construction; `Weak` avoids a self-referential cycle.
    me: Weak<dyn WorkflowHost>,
}

impl WorkflowRunHost {
    fn build_request(
        &self,
        prompt: String,
        opts: &WorkflowAgentOpts,
        attempt_abort: coco_tool_runtime::TurnAbortSignal,
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
            input: AgentSpawnInput {
                prompt,
                description: Some(
                    opts.label
                        .clone()
                        .unwrap_or_else(|| "workflow step".to_string()),
                ),
                subagent_type: opts.agent_type.clone(),
                definition,
                output_schema: opts.schema.clone().map(std::sync::Arc::new),
                ..Default::default()
            },
            execution: AgentSpawnExecution {
                // Foreground: we await the result inline. The universal
                // subagent deny-list already blocks Agent + Workflow.
                run_in_background: false,
                spawn_mode: SpawnMode::Fresh,
                isolation,
                ..Default::default()
            },
            permissions: AgentSpawnPermissions {
                mode: Some(coco_permissions::resolve_subagent_mode(
                    ctx.parent_mode,
                    None,
                )),
                ..Default::default()
            },
            inheritance: AgentSpawnInheritance {
                features: Some(ctx.features.clone()),
                skill_overrides: Some(ctx.skill_overrides.clone()),
                tool_overrides: Some(ctx.tool_overrides.clone()),
                parent_tool_filter: Some(ctx.parent_tool_filter.clone()),
                active_shell_tool: ctx.active_shell_tool,
                mcp_tool_exposure: ctx.mcp_tool_exposure,
                mcp_server_tool_exposure: ctx.mcp_server_tool_exposure.clone(),
                ..Default::default()
            },
            routing: AgentSpawnRouting {
                session_id: ctx.session_id.clone(),
                parent_turn_abort: Some(attempt_abort),
                ..Default::default()
            },
            telemetry: AgentSpawnTelemetry {
                tool_use_id: ctx.tool_use_id.clone(),
                invoking_agent_id: ctx.invoking_agent_id.clone(),
                log_assistant_responses: ctx.log_assistant_responses,
                is_non_interactive: true,
                ..Default::default()
            },
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

#[async_trait::async_trait(?Send)]
impl WorkflowHost for WorkflowRunHost {
    async fn run_agent(
        &self,
        prompt: String,
        opts: WorkflowAgentOpts,
    ) -> Result<WorkflowAgentResult, String> {
        // Bound concurrent subagent spawns: each agent() call queues on the
        // shared FIFO semaphore. Held across every retry; released on return.
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| format!("workflow concurrency semaphore closed: {e}"))?;

        // Per-agent stall watchdog (CC parity): a spawn that produces no result
        // within `stall` is aborted and retried up to WORKFLOW_STALL_RETRY
        // times. Only this slot is reclaimed — the whole run keeps going. On
        // exhaustion we return Err, which the engine maps to a rejected promise
        // (→ null in the surrounding parallel/pipeline).
        let stall_ms = opts
            .stall_ms
            .filter(|ms| *ms > 0)
            .unwrap_or(WORKFLOW_STALL_MS_DEFAULT);
        let stall = std::time::Duration::from_millis(stall_ms.max(0) as u64);

        let mut attempt = 0i32;
        loop {
            attempt += 1;
            // Fresh per-attempt abort built from a CHILD of the shared workflow
            // abort token: a whole-run cancel propagates down (cancels this
            // in-flight subagent), but cancelling this child on a stall aborts
            // only this attempt — never the parent run.
            let attempt_token = self.spawn_ctx.workflow_abort.token().child_token();
            let attempt_abort =
                coco_tool_runtime::TurnAbortSignal::from_token(attempt_token.clone());
            let request = self.build_request(prompt.clone(), &opts, attempt_abort)?;
            let agent = self.agent.clone();
            // Spawn on the main runtime (the agent system runs there); await the
            // result from this dedicated engine thread, bounded by the stall.
            let spawn = self
                .main_handle
                .spawn(async move { agent.spawn_agent(request).await });
            match tokio::time::timeout(stall, spawn).await {
                Ok(join_result) => {
                    let response = join_result
                        .map_err(|e| format!("workflow subagent task join error: {e}"))??;
                    return convert_response(response, &opts);
                }
                Err(_elapsed) => {
                    // Stall: abort this attempt's subagent. Retry if budget
                    // remains; otherwise surface a terminal failure.
                    attempt_token.cancel();
                    if attempt >= WORKFLOW_STALL_RETRY {
                        return Err(format!(
                            "workflow subagent stalled ({stall_ms} ms) after \
                             {WORKFLOW_STALL_RETRY} attempts"
                        ));
                    }
                    self.push_progress(WorkflowProgressEvent::WorkflowLog {
                        message: format!("retrying ({attempt}/{WORKFLOW_STALL_RETRY})"),
                    });
                }
            }
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

    fn budget_exhausted(&self) -> bool {
        self.budget_total_tokens()
            .is_some_and(|total| total > 0 && self.budget_spent_tokens() >= total)
    }

    async fn cached_agent_result(&self, key: &AgentCacheKey) -> Option<serde_json::Value> {
        self.journal.lookup(key).await
    }

    async fn record_agent_result(&self, key: &AgentCacheKey, value: &serde_json::Value) {
        self.journal.record(key, value).await;
    }

    async fn run_nested_workflow(
        &self,
        name_or_ref: String,
        args: serde_json::Value,
        depth: i32,
    ) -> Result<serde_json::Value, String> {
        // Resolve the child source: a `.ts`/`.js` ref is a `{scriptPath}`,
        // anything else is a saved-workflow name (matched against parsed
        // meta.name). Resolution + parse live in `coco_workflow`, which is only
        // reachable from this host crate — that is why nesting is host-backed.
        let source_input = if is_script_path_ref(&name_or_ref) {
            coco_workflow::WorkflowSourceInput {
                script_path: Some(PathBuf::from(&name_or_ref)),
                cwd: self.spawn_ctx.cwd.clone(),
                ..Default::default()
            }
        } else {
            coco_workflow::WorkflowSourceInput {
                name: Some(name_or_ref.clone()),
                cwd: self.spawn_ctx.cwd.clone(),
                ..Default::default()
            }
        };
        let spec = coco_workflow::resolve_workflow_source(source_input)
            .map_err(|error| format!("workflow('{name_or_ref}') was not launched: {error}"))?;
        // The child body has determinism checked (it is a freshly-resolved
        // source, like a top-level named/scriptPath launch).
        let script = coco_workflow::parse_workflow_script(&spec.source, true)
            .map_err(|error| format!("workflow('{name_or_ref}') was not launched: {error}"))?;

        // Re-enter the engine on THIS thread with the SAME host Arc so the child
        // shares the parent's semaphore, token budget, journal, abort signal, and
        // agent counter (no fresh governance is allocated). The child runs at
        // `depth >= 1`, so its own `workflow()` throws the one-level guard.
        let host = self
            .me
            .upgrade()
            .ok_or_else(|| "workflow host was dropped".to_string())?;
        let cancel = self.spawn_ctx.workflow_abort.token();
        WorkflowEngine::run(
            script.script_body,
            args,
            host,
            cancel,
            WORKFLOW_SYNC_EVAL_BUDGET,
            depth,
        )
        .await
        .map_err(|error| error.to_string())
    }
}

/// Whether a `workflow(nameOrRef)` argument is a `{scriptPath}` reference rather
/// than a saved-workflow name: a path ending in a workflow extension. Names are
/// matched against parsed `meta.name`, never used to build a path, so anything
/// that is not an explicit script path is treated as a name.
fn is_script_path_ref(name_or_ref: &str) -> bool {
    std::path::Path::new(name_or_ref)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ts") || ext.eq_ignore_ascii_case("js"))
}

/// Current-thread Tokio runtime paired with a `LocalSet` for the `!Send`
/// QuickJS workflow engine. Keeping the constructor private makes the
/// workflow host the single boundary that can drive these futures.
struct LocalWorkflowRuntime {
    runtime: tokio::runtime::Runtime,
    local: tokio::task::LocalSet,
}

impl LocalWorkflowRuntime {
    fn new() -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(Self {
            runtime,
            local: tokio::task::LocalSet::new(),
        })
    }

    fn block_on<F>(&self, future: F) -> F::Output
    where
        F: std::future::Future,
    {
        self.local.block_on(&self.runtime, future)
    }
}

/// Small `!Send` future used by tests to guard the local-runtime boundary.
#[cfg(test)]
struct LocalOnlyReady(std::marker::PhantomData<std::rc::Rc<()>>);

#[cfg(test)]
impl LocalOnlyReady {
    fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

#[cfg(test)]
impl std::future::Future for LocalOnlyReady {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::task::Poll::Ready(())
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
    journal: Arc<WorkflowJournal>,
) {
    let thread = std::thread::Builder::new()
        .name(format!("workflow-{task_id}"))
        .spawn(move || {
            // `new_cyclic` lets the host hold a `Weak` to itself so
            // `run_nested_workflow` can re-enter the engine with the SAME host
            // Arc — the mechanism that shares all governance with a child run.
            let host: Arc<WorkflowRunHost> = Arc::new_cyclic(|me| WorkflowRunHost {
                agent,
                task_handle: task_handle.clone(),
                task_id: task_id.clone(),
                main_handle,
                spawn_ctx,
                budget_spent_tokens: AtomicI64::new(0),
                semaphore: Arc::new(Semaphore::new(workflow_local_concurrency())),
                journal,
                // `new_cyclic` hands a `Weak<WorkflowRunHost>`; coerce to the
                // trait-object weak the field stores.
                me: me.clone() as Weak<dyn WorkflowHost>,
            });
            let host: Arc<dyn WorkflowHost> = host;
            let runtime = match LocalWorkflowRuntime::new() {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!(target: "coco::workflow", %error, "failed to build workflow runtime");
                    return;
                }
            };
            runtime.block_on(async move {
                let outcome = WorkflowEngine::run(
                    script,
                    args,
                    host,
                    cancel,
                    WORKFLOW_SYNC_EVAL_BUDGET,
                    /*depth*/ 0,
                )
                .await;
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

/// Convert a completed `AgentSpawnResponse` into a `WorkflowAgentResult`.
/// Honours the structured-output contract: schema-forced spawns must surface
/// the validated tool-call input on `structured_output`.
fn convert_response(
    response: coco_tool_runtime::AgentSpawnResponse,
    opts: &WorkflowAgentOpts,
) -> Result<WorkflowAgentResult, String> {
    match response.status {
        AgentSpawnStatus::Completed => {
            let model = response.model.clone();
            let tokens = response.input_tokens + response.output_tokens;
            let tool_calls = i32::try_from(response.total_tool_use_count).ok();
            let duration_ms = Some(response.duration_ms);
            let text = response.result.unwrap_or_default();
            let value = if opts.schema.is_some() {
                response.structured_output.ok_or_else(|| {
                    "agent({schema}): subagent completed without calling StructuredOutput \
                     (after in-conversation nudge)"
                        .to_string()
                })?
            } else {
                serde_json::Value::String(text)
            };
            Ok(WorkflowAgentResult {
                value,
                model,
                tokens: Some(tokens),
                tool_calls,
                duration_ms,
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
