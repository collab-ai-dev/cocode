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
use coco_workflow_runtime::WorkflowAgentOutcome;
use coco_workflow_runtime::WorkflowAgentResult;
use coco_workflow_runtime::WorkflowEngine;
use coco_workflow_runtime::WorkflowHost;
use coco_workflow_runtime::WorkflowRun;
use coco_workflow_runtime::WorkflowRunState;
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
    /// The launching turn's permission context. Carried whole (not just its
    /// `mode`) because `agent({agentType})` resolves against the same
    /// `Agent(<type>)` rule surface as the Agent tool, and the dispatch screen
    /// reads its mode.
    pub permission_context: coco_types::ToolPermissionContext,
    /// The launching agent's transcript, so the dispatch screen's classifier
    /// judges each `agent()` request in the context that produced it.
    pub messages: Arc<Vec<Arc<coco_messages::Message>>>,
    /// Screens each `agent()` dispatch — see
    /// [`coco_tool_runtime::subagent_screen`].
    pub subagent_screen: coco_tool_runtime::SubagentDispatchScreenHandle,
    /// The session's Agent dispatch budget. Shared with the `Agent` tool so a
    /// workflow cannot spawn past a ceiling the equivalent tool call honours.
    pub session_usage: Arc<coco_tool_runtime::SessionUsageLimits>,
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
    /// Run-scoped counters (agent ordinal, phase table, replay cursor). Held
    /// here — not inside the engine — so `run_nested_workflow` hands the child
    /// engine the SAME state and the child continues the parent's numbering
    /// instead of restarting the lifetime agent cap.
    run_state: Arc<WorkflowRunState>,
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
                    ctx.permission_context.mode,
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
        let requested = opts
            .agent_type
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let mut definition = match requested {
            Some(name) => self.resolve_requested_agent_type(name)?,
            None => self.default_agent_definition(),
        };

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

    /// Resolve an explicit `agent({agentType})` against the live catalog and the
    /// Agent tool's permission rules.
    ///
    /// A workflow script is model-authored code and the Workflow tool is
    /// approved once, so reading the unfiltered catalog here would make
    /// `deny: ["Agent(deploy-bot)"]` unenforceable — a script could ask for the
    /// denied type and get it. Unknown types are a hard error rather than a
    /// silent fall back to general-purpose: a typo'd agentType that quietly runs
    /// a different agent is worse than a failed slot the script can see.
    fn resolve_requested_agent_type(
        &self,
        name: &str,
    ) -> Result<coco_types::AgentDefinition, String> {
        if let Some(denied) = crate::tools::agent::agent_tool::find_agent_deny_rule(
            &self.spawn_ctx.permission_context,
            name,
        ) {
            return Err(format!(
                "agent({{agentType}}): '{name}' is denied by permission rule '{tool}({name})' from {source:?}.",
                tool = coco_types::ToolName::Agent.as_str(),
                source = denied.source,
            ));
        }
        let Some(catalog) = self.spawn_ctx.agent_catalog.as_ref() else {
            return Err(format!(
                "agent({{agentType}}): agent type '{name}' not found — no agent catalog is loaded."
            ));
        };
        catalog.find_active(name).cloned().ok_or_else(|| {
            // List only the types the caller could actually have used; naming
            // denied ones would leak the existence of restricted agents.
            let available: Vec<&str> = catalog
                .active()
                .map(|def| def.name.as_str())
                .filter(|candidate| {
                    crate::tools::agent::agent_tool::find_agent_deny_rule(
                        &self.spawn_ctx.permission_context,
                        candidate,
                    )
                    .is_none()
                })
                .collect();
            format!(
                "agent({{agentType}}): agent type '{name}' not found. Available agents: {}",
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                }
            )
        })
    }

    /// The definition an `agent()` call with no `agentType` runs under: the
    /// catalog's general-purpose entry, or a bare synthesized one when no
    /// catalog is loaded.
    fn default_agent_definition(&self) -> coco_types::AgentDefinition {
        let name = coco_types::SubagentType::GeneralPurpose.as_str();
        self.spawn_ctx
            .agent_catalog
            .as_ref()
            .and_then(|catalog| catalog.find_active(name).cloned())
            .unwrap_or_else(|| coco_types::AgentDefinition {
                agent_type: name.parse().expect("AgentTypeId::from_str is Infallible"),
                name: name.to_string(),
                source: coco_types::AgentSource::BuiltIn,
                ..Default::default()
            })
    }
}

#[async_trait::async_trait(?Send)]
impl WorkflowHost for WorkflowRunHost {
    async fn run_agent(
        &self,
        prompt: String,
        opts: WorkflowAgentOpts,
        started: coco_workflow_runtime::WorkflowAgentStarted<'_>,
    ) -> Result<WorkflowAgentOutcome, String> {
        // Compile the script's schema up front so a bad one fails the `agent()`
        // call the script can see, rather than surfacing turns later as a
        // subagent that never called StructuredOutput. It also bounds what the
        // dispatch screen below is asked to reason about. The compiled
        // validator is discarded — the subagent builds its own — but the
        // meta-validation and the size bounds are what this is for.
        if let Some(schema) = opts.schema.as_ref() {
            coco_tool_runtime::ToolInputSchema::from_value(schema.clone()).map_err(|error| {
                format!("agent({{schema}}) received an invalid JSON Schema: {error}")
            })?;
        }

        // Auto-mode dispatch screen. This call never passed through the tool
        // pipeline — it is a script call, not a model tool call — so without
        // this the Workflow tool would be a way to dispatch subagents that the
        // equivalent `Agent` call could not. Runs before the permit so a
        // refused dispatch never occupies a concurrency slot.
        let ctx = &self.spawn_ctx;
        let dispatch = coco_tool_runtime::SubagentDispatch {
            prompt: &prompt,
            subagent_type: opts.agent_type.as_deref(),
            output_schema: opts.schema.as_ref(),
            permission_context: &ctx.permission_context,
            messages: &ctx.messages,
            cwd: ctx.cwd.as_deref().and_then(std::path::Path::to_str),
        };
        if let coco_tool_runtime::SubagentDispatchVerdict::Block { reason } =
            ctx.subagent_screen.screen(dispatch).await
        {
            return Ok(WorkflowAgentOutcome::Refused {
                reason: format!("blocked by safety classifier: {reason}"),
                blocked: true,
            });
        }

        // Same reasoning as the screen above: this dispatch never passed the
        // tool pipeline, so the session spawn budget the `Agent` tool charges
        // has to be charged here too — otherwise a workflow is a way to spawn
        // subagents past a ceiling the equivalent `Agent` call would refuse.
        // Charged once per `agent()` call, before the permit: the stall-retry
        // loop below re-runs one logical dispatch, not a new one.
        if let coco_tool_runtime::UsageLimitDecision::Exhausted { used, limit } =
            ctx.session_usage.try_record_agent_spawn()
        {
            return Ok(WorkflowAgentOutcome::Refused {
                reason: format!(
                    "session subagent spawn limit reached ({used} of {limit} agents spawned \
                     in this session)"
                ),
                blocked: true,
            });
        }

        // Bound concurrent subagent spawns: each agent() call queues on the
        // shared FIFO semaphore. Held across every retry; released on return.
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| format!("workflow concurrency semaphore closed: {e}"))?;
        // The wait is over — flip the row from queued to running. Retries below
        // reuse the same permit, so this fires exactly once per `agent()` call.
        started();

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
                    return convert_response(response, &opts).map(WorkflowAgentOutcome::Completed);
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

    fn cached_agent_result(&self, key: &AgentCacheKey) -> Option<serde_json::Value> {
        let hit = self.journal.lookup(key);
        if hit.is_none() {
            // Crash-forensics breadcrumb only (hydration ignores `started`
            // lines), so it is fire-and-forget on the main runtime rather than
            // an await on the pre-spawn path.
            let journal = self.journal.clone();
            let key = key.clone();
            self.main_handle.spawn(async move {
                journal.record_started(&key).await;
            });
        }
        hit
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

        // Re-enter the engine on THIS thread with the SAME host Arc AND the SAME
        // run state, so the child shares the parent's semaphore, token budget,
        // journal, abort signal, agent ordinal, phase table and replay cursor
        // (no fresh governance is allocated). The child runs at `depth >= 1`, so
        // its own `workflow()` throws the one-level guard.
        let host = self
            .me
            .upgrade()
            .ok_or_else(|| "workflow host was dropped".to_string())?;
        // The child's own `meta.name` — not the caller's `nameOrRef`, which may
        // be a path — names the group its agents render under.
        let child_group = self.run_state.next_child_group(&script.meta.name);
        WorkflowEngine::run(WorkflowRun {
            script: script.script_body,
            args,
            host,
            state: self.run_state.clone(),
            cancel: self.spawn_ctx.workflow_abort.token(),
            sync_eval_budget: WORKFLOW_SYNC_EVAL_BUDGET,
            depth,
            child_group: Some(child_group),
        })
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

/// Everything the engine thread needs beyond the script itself. Bundled so the
/// launch seam stays a two-argument call as the run's governance grows.
pub(crate) struct WorkflowLaunch {
    pub args: serde_json::Value,
    pub agent: AgentHandleRef,
    pub task_handle: TaskHandleRef,
    pub task_id: String,
    pub cancel: CancellationToken,
    pub spawn_ctx: WorkflowSpawnContext,
    pub main_handle: tokio::runtime::Handle,
    pub journal: Arc<WorkflowJournal>,
    /// `meta.phases[].title`, pre-interned so declared phases hold indices
    /// `1..=N` before any agent runs and the progress tree renders its full
    /// skeleton immediately.
    pub seed_phases: Vec<String>,
    /// This run's `wf_…` id, quoted back in the completion notification's
    /// resume instruction.
    pub run_id: String,
    /// `journal.jsonl` for this run, named in the completion notification so the
    /// model can read each agent's actual return value.
    pub journal_path: Option<PathBuf>,
}

/// Launch the workflow engine on a dedicated OS thread (the engine is `!Send`).
/// Fire-and-forget: returns immediately; the thread runs the script to
/// completion, then marks the task terminal. `agent()`/progress bridge to
/// `main_handle`.
pub(crate) fn spawn_workflow_engine(script: String, launch: WorkflowLaunch) {
    let WorkflowLaunch {
        args,
        agent,
        task_handle,
        task_id,
        cancel,
        spawn_ctx,
        main_handle,
        journal,
        seed_phases,
        run_id,
        journal_path,
    } = launch;
    let thread = std::thread::Builder::new()
        .name(format!("workflow-{task_id}"))
        .spawn(move || {
            // `new_cyclic` lets the host hold a `Weak` to itself so
            // `run_nested_workflow` can re-enter the engine with the SAME host
            // Arc — the mechanism that shares all governance with a child run.
            let run_state = Arc::new(WorkflowRunState::new(seed_phases));
            let host: Arc<WorkflowRunHost> = Arc::new_cyclic(|me| WorkflowRunHost {
                agent,
                task_handle: task_handle.clone(),
                task_id: task_id.clone(),
                main_handle,
                spawn_ctx,
                budget_spent_tokens: AtomicI64::new(0),
                semaphore: Arc::new(Semaphore::new(workflow_local_concurrency())),
                journal,
                run_state: run_state.clone(),
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
                let outcome = WorkflowEngine::run(WorkflowRun {
                    script,
                    args,
                    host,
                    state: run_state,
                    cancel,
                    sync_eval_budget: WORKFLOW_SYNC_EVAL_BUDGET,
                    depth: 0,
                    child_group: None,
                })
                .await;
                let census = agent_census(task_handle.as_ref(), &task_id).await;
                match outcome {
                    Ok(value) => {
                        task_handle
                            .mark_completed(
                                &task_id,
                                AgentCompletionPayload {
                                    result: Some(render_result(&value)),
                                    diagnostics: Some(completed_diagnostics(
                                        &run_id,
                                        journal_path.as_deref(),
                                        &census,
                                    )),
                                    ..AgentCompletionPayload::default()
                                },
                            )
                            .await;
                    }
                    Err(error) => {
                        task_handle
                            .mark_failed_with_diagnostics(
                                &task_id,
                                &error.to_string(),
                                failed_diagnostics(&run_id, journal_path.as_deref(), &census),
                            )
                            .await;
                    }
                }
            });
        });
    if let Err(error) = thread {
        tracing::error!(target: "coco::workflow", %error, "failed to spawn workflow engine thread");
    }
}

/// How the run's `agent()` calls ended, counted off the progress array.
#[derive(Debug, Default, PartialEq, Eq)]
struct AgentCensus {
    done: i32,
    errored: i32,
    skipped: i32,
    /// Refused by the auto-mode dispatch screen. Separate from `errored` so a
    /// policy block never reads as an agent that tried and failed.
    blocked: i32,
    /// Done agents whose result was absent or an empty container. Counted
    /// separately because "12 done, 12 empty" and "12 done, 1 empty" call for
    /// completely different follow-ups: the first says the fan-out found nothing
    /// anywhere (suspect the prompt), the second says one agent came back empty
    /// (probably genuine).
    empty: i32,
}

impl AgentCensus {
    fn render(&self) -> String {
        format!(
            "agents_done={} agents_error={} agents_skipped={} agents_blocked={} \
             agents_empty_result={}",
            self.done, self.errored, self.skipped, self.blocked, self.empty
        )
    }
}

/// Whether a result preview is one of the shapes an agent produces when it found
/// nothing: `[]`, `{}`, or a single key holding an empty array. Deliberately not
/// a general emptiness test — `{"count": 0}` is a real answer, and treating it
/// as empty would blunt the signal.
fn is_empty_result_preview(preview: Option<&str>) -> bool {
    let Some(preview) = preview else {
        return true;
    };
    let trimmed = preview.trim();
    if trimmed.is_empty() || trimmed == "[]" || trimmed == "{}" {
        return true;
    }
    let Some(inner) = trimmed
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
    else {
        return false;
    };
    let Some((key, value)) = inner.split_once(':') else {
        return false;
    };
    let key = key.trim();
    key.len() >= 2
        && key.starts_with('"')
        && key.ends_with('"')
        && !key[1..key.len() - 1].contains('"')
        && value.trim() == "[]"
}

async fn agent_census(
    task_handle: &dyn coco_tool_runtime::TaskHandle,
    task_id: &str,
) -> AgentCensus {
    let Ok(state) = task_handle.get_task_status(task_id).await else {
        return AgentCensus::default();
    };
    let Some(extras) = state.extras.local_workflow_extras() else {
        return AgentCensus::default();
    };
    let mut census = AgentCensus::default();
    for event in &extras.workflow_progress {
        let coco_types::WorkflowProgressEvent::WorkflowAgent {
            state,
            blocked,
            skipped,
            result_preview,
            ..
        } = event
        else {
            continue;
        };
        match state {
            coco_types::WorkflowAgentState::Done => {
                census.done += 1;
                if is_empty_result_preview(result_preview.as_deref()) {
                    census.empty += 1;
                }
            }
            coco_types::WorkflowAgentState::Error if *blocked => census.blocked += 1,
            coco_types::WorkflowAgentState::Error if *skipped => census.skipped += 1,
            coco_types::WorkflowAgentState::Error => census.errored += 1,
            coco_types::WorkflowAgentState::Start | coco_types::WorkflowAgentState::Progress => {}
        }
    }
    census
}

/// The `<diagnostics>` block for a run that finished.
///
/// A workflow can complete and return `[]` for two very different reasons — the
/// agents found nothing, or the agents returned nothing — and the run's own
/// return value cannot tell them apart. The journal can, so the model is pointed
/// at it before it draws a conclusion.
fn completed_diagnostics(
    run_id: &str,
    journal_path: Option<&std::path::Path>,
    census: &AgentCensus,
) -> String {
    let mut text = String::new();
    if let Some(path) = journal_path {
        text.push_str(&format!(
            "Per-agent results: {} — one JSON line per completed agent with its full return value.\n\
             If the result above is empty or unexpected, Read this file BEFORE diagnosing — do not \
             assume the agents returned non-empty results.\n",
            path.display()
        ));
    }
    text.push_str(&format!("Agent outcomes: {}\n", census.render()));
    text.push_str(&format!(
        "To re-run with edited post-processing: Workflow({{resumeFromRunId: \"{run_id}\"}}) — agents \
         whose prompt and opts are unchanged replay from the journal instead of re-spawning."
    ));
    text
}

/// The `<diagnostics>` block for a run that failed or was stopped: the literal
/// next action, so the model is not left reconstructing a resume call from the
/// tool schema.
fn failed_diagnostics(
    run_id: &str,
    journal_path: Option<&std::path::Path>,
    census: &AgentCensus,
) -> String {
    let mut text = format!(
        "To resume after fixing the script, call: Workflow({{resumeFromRunId: \"{run_id}\"}}) — \
         completed agents replay from the journal, so only the failed tail re-runs.\n"
    );
    if let Some(path) = journal_path {
        text.push_str(&format!("Per-agent results so far: {}\n", path.display()));
    }
    text.push_str(&format!("Agent outcomes: {}", census.render()));
    text
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
