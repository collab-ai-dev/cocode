use std::sync::Arc;

use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::ToolRegistry;
use tokio_util::sync::CancellationToken;

use super::EnginePersistenceMode;
use super::SessionRuntime;

impl SessionRuntime {
    /// Install the agent-spawn handle on this runtime. Called once
    /// after `build()` returns the `Arc<Self>`. The handle is
    /// late-bound because the adapter inside it needs to capture
    /// `Arc<Self>` to drive per-spawn engine builds - calling this
    /// from inside `build()` would create a cycle.
    pub async fn attach_agent_handle(&self, handle: AgentHandleRef) {
        *self.agent_handle.write().await = Some(handle.clone());
        if let Some(runtime) = &self.memory_runtime {
            runtime.install_agent(handle.clone());
        }
        if let Some(rt) = &self.skill_review_runtime {
            rt.install_agent(handle);
        }
    }

    /// Resolve this session's team snapshot for the team-coordination
    /// reminder. A teammate session uses its own identity (env/context);
    /// a leader session uses the roster active team. `None` when not in a
    /// team. Computed per-turn in `wire_engine` so the app-query
    /// `SwarmAdapter` stays coordinator-free.
    pub(super) async fn resolve_team_snapshot(
        &self,
    ) -> Option<coco_system_reminder::TeamContextSnapshot> {
        let (agent_id, agent_name, team_name) =
            if let Some(id) = coco_coordinator::identity::resolve_teammate_identity() {
                (id.agent_id, id.agent_name, id.team_name)
            } else {
                let handle = self.current_agent_handle().await?;
                let team = handle.active_team_name().await?;
                (format!("team-lead@{team}"), "team-lead".to_string(), team)
            };
        let task_list_id = coco_coordinator::types::sanitize_name(&team_name);
        Some(coco_system_reminder::TeamContextSnapshot {
            team_config_path: coco_coordinator::team_file::get_team_file_path(&team_name)
                .display()
                .to_string(),
            task_list_path: self
                .config_home
                .join("tasks")
                .join(coco_tasks::task_list::sanitize_path_component(
                    &task_list_id,
                ))
                .display()
                .to_string(),
            agent_id,
            agent_name,
            team_name,
        })
    }

    /// Install the skill-execution handle (`QuerySkillRuntime`). Late-bound
    /// alongside `attach_agent_handle` because the real impl wraps the same
    /// subagent `AgentQueryEngineRef` the swarm handle uses. Once set,
    /// `wire_engine` installs it on every per-turn engine so the model's
    /// `SkillTool` and user-typed fork-mode `/slash` skills resolve.
    pub async fn attach_skill_handle(&self, handle: coco_tool_runtime::SkillHandleRef) {
        *self.skill_handle.write().await = Some(handle);
    }

    /// Snapshot the installed skill handle, if any. Used by the TUI
    /// slash-command dispatch to run user-typed fork-mode skills through
    /// the same `SkillHandle` the model's `SkillTool` uses.
    pub async fn skill_handle(&self) -> Option<coco_tool_runtime::SkillHandleRef> {
        self.skill_handle.read().await.clone()
    }

    /// The shared Bash-handle cell threaded into `QuerySkillRuntime` so the
    /// skill paths run the same permission-checked in-prompt shell as
    /// slash-command handlers. Clone shares the same `Arc` cell.
    pub(crate) fn skill_bash_cell(
        &self,
    ) -> Arc<std::sync::RwLock<Option<Arc<dyn coco_skills::shell_exec::BashToolHandle>>>> {
        self.skill_bash_cell.clone()
    }

    /// Run a user-typed fork-mode skill (`/<name>`) through the installed
    /// `SkillHandle`: the skill body runs as a subagent and its final text
    /// is returned for the caller to inject as a `<local-command-stdout>`
    /// block - no follow-up main-model query.
    /// The gate marks the skill as user-invoked (`typed_slashes_in_turn`),
    /// so it bypasses the `disable_model_invocation` author lock and the
    /// `user-invocable-only` override. Inheritance is taken from the live
    /// engine config so the subagent runs with the session's features /
    /// permission mode / tool overrides. Returns `Err` when no skill
    /// runtime is installed or the fork fails.
    pub async fn invoke_skill_fork(&self, name: &str, args: &str) -> Result<String, String> {
        let handle = self
            .skill_handle()
            .await
            .ok_or_else(|| "no skill runtime installed".to_string())?;
        let cfg = self.current_engine_config().await;
        let inherit = coco_tool_runtime::SubagentInheritance {
            session_id: cfg.session_id.clone(),
            permission_mode: cfg.permission_mode,
            features: Some(cfg.features.clone()),
            tool_overrides: Some(cfg.tool_overrides.clone()),
            active_shell_tool: cfg.active_shell_tool,
            use_auto_mode_during_plan: cfg.use_auto_mode_during_plan,
            log_assistant_responses: cfg.log_assistant_responses,
            parent_tool_filter: None,
            // Fork-mode skill subagents count toward the same depth cap as
            // other forked subagents.
            parent_query_depth: cfg.query_depth,
        };
        let gate = coco_tool_runtime::SkillGateContext {
            overrides: cfg.skill_overrides.clone(),
            typed_slashes_in_turn: std::iter::once(name.to_string()).collect(),
        };
        match handle.invoke_skill(name, args, inherit, gate).await {
            Ok(coco_tool_runtime::SkillInvocationResult::Forked { output, .. }) => Ok(output),
            // A fork-context skill always resolves to `Forked`; the inline
            // arm is defensive (e.g. a skill whose context flipped between
            // registry snapshot and dispatch) - surface its summary.
            Ok(coco_tool_runtime::SkillInvocationResult::Inline { summary, .. }) => Ok(summary),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Interrupt an in-process teammate's current turn without
    /// cancelling the teammate lifecycle.
    pub async fn interrupt_agent_current_work(&self, agent_id: &str) -> Result<bool, String> {
        let handle = self
            .agent_handle
            .read()
            .await
            .clone()
            .unwrap_or_else(|| self.swarm_agent_handle.clone());
        handle.interrupt_agent_current_work(agent_id).await
    }

    /// Install the post-turn fork dispatcher (D1/D2). Late-bound for
    /// the same Arc-cycle reason as `attach_agent_handle`: the
    /// dispatcher impl captures `Arc<Self>` to build per-fork engines.
    pub async fn attach_fork_dispatcher(
        &self,
        dispatcher: coco_query::forked_agent::ForkDispatcherRef,
    ) {
        *self.fork_dispatcher.write().await = Some(dispatcher);
    }

    /// Install the runtime-backed Agent hook runner onto the shared
    /// LLM hook handle. Called after `SessionRuntime::build` returns
    /// because the runner captures `Arc<SessionRuntime>`.
    pub async fn attach_hook_agent_runner(&self, runner: coco_query::hook_llm::HookAgentRunnerRef) {
        self.hook_llm_handle.install_agent_runner(runner).await;
    }

    /// Snapshot the registered tool set for scoped child registries.
    pub(crate) fn registered_tools(&self) -> Vec<Arc<dyn coco_tool_runtime::DynTool>> {
        self.tools.all()
    }

    /// Build an engine with caller-supplied scoped registries, then
    /// apply the standard wiring. Used by the hook-agent runner, a scoped
    /// child engine that must not write to the main-session transcript,
    /// usage tracker, or file-history sink - so it wires as `Fork`.
    pub(crate) async fn build_engine_from_config_with_registries(
        &self,
        config: QueryEngineConfig,
        cancel: CancellationToken,
        tools: Arc<ToolRegistry>,
        hooks: Option<Arc<coco_hooks::HookRegistry>>,
    ) -> QueryEngine {
        let engine = QueryEngine::new(config, self.model_runtimes.clone(), tools, cancel, hooks);
        self.wire_engine(engine, None, EnginePersistenceMode::Fork)
            .await
    }

    /// Read the currently installed fork dispatcher. Returns `None`
    /// before bootstrap installs one (or in unit tests). Used by SDK
    /// runners that want to dispatch a fork outside of the engine's
    /// post-turn hook (`/btw` over the SDK protocol).
    pub async fn current_fork_dispatcher(
        &self,
    ) -> Option<coco_query::forked_agent::ForkDispatcherRef> {
        self.fork_dispatcher.read().await.clone()
    }

    /// Read the most recent turn's cache-safe params. `None` before the
    /// first turn finalises (or after `/clear`). The TUI `/btw` dispatch
    /// uses this when present and falls back to rebuilding from transcript.
    pub async fn last_cache_safe_params(&self) -> Option<coco_types::CacheSafeParams> {
        let handle = self.last_engine_cache_handle.read().await.clone();
        match handle {
            Some(h) => h.read().await.clone(),
            None => None,
        }
    }

    /// Build fresh cache params from the current session config and transcript.
    /// This is the `/btw` fallback when no post-turn cache slot exists yet.
    pub async fn fallback_cache_safe_params(&self) -> coco_types::CacheSafeParams {
        let cfg = self.current_engine_config().await;
        let snapshot = self
            .model_runtimes
            .snapshot_for_role(coco_types::ModelRole::Main)
            .ok();
        let provider = snapshot
            .as_ref()
            .map(|s| s.provider.clone())
            .unwrap_or_default();
        let slot_effort = snapshot.and_then(|s| s.role_effort);
        let history = {
            let guard = self.history.lock().await;
            guard.snapshot()
        };
        coco_query::QueryEngine::cache_safe_params_from_parts(&cfg, provider, slot_effort, &history)
    }

    /// Install the background task runtime. Called once during CLI
    /// bootstrap; the same `Arc` flows into `SwarmAgentHandle` for
    /// the registration side. Idempotent - re-attaching replaces.
    pub async fn attach_task_runtime(&self, rt: Arc<crate::task_runtime::TaskRuntime>) {
        *self.task_runtime.write().await = Some(rt);
    }

    /// Read the installed task runtime. `None` when no production
    /// runtime is wired (tests, headless paths that don't use bg
    /// AgentTool). Used by `agent_handle_factory` to share the same
    /// instance with `SwarmAgentHandle`.
    pub async fn current_task_runtime(&self) -> Option<Arc<crate::task_runtime::TaskRuntime>> {
        self.task_runtime.read().await.clone()
    }

    pub async fn attach_task_list(&self, handle: coco_tool_runtime::TaskListHandleRef) {
        *self.task_list.write().await = Some(handle);
    }

    pub async fn attach_team_task_list_router(
        &self,
        router: coco_tool_runtime::TeamTaskListRouterRef,
    ) {
        *self.team_task_list_router.write().await = Some(router);
    }

    pub async fn current_task_list(&self) -> Option<coco_tool_runtime::TaskListHandleRef> {
        self.task_list.read().await.clone()
    }

    pub async fn current_team_task_list_router(
        &self,
    ) -> Option<coco_tool_runtime::TeamTaskListRouterRef> {
        self.team_task_list_router.read().await.clone()
    }

    /// Install the per-agent transcript / metadata store used for
    /// background AgentTool resume. Late-bind: same lifecycle as
    /// `attach_task_runtime`. `agent_handle_factory` reads this and
    /// forwards onto `SwarmAgentHandle::set_transcript_store`.
    pub async fn attach_agent_transcript_store(
        &self,
        store: coco_tool_runtime::AgentTranscriptStoreRef,
    ) {
        *self.agent_transcript_store.write().await = Some(store);
    }

    /// Read the installed agent-transcript store.
    pub async fn current_agent_transcript_store(
        &self,
    ) -> Option<coco_tool_runtime::AgentTranscriptStoreRef> {
        self.agent_transcript_store.read().await.clone()
    }
}
