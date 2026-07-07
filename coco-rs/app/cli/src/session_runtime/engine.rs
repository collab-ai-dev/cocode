use std::sync::Arc;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use coco_messages::MessageHistory;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_query::SessionStartHookSideEffectSink;
use coco_query::SessionStartHookSideEffects;
use coco_tool_runtime::TurnAbortSignal;
use coco_types::ToolAppState;

use super::EnginePersistenceMode;
use super::SessionRuntime;
use super::hooks::FileWatchRegistrationContext;

struct QuerySessionStartHookSink {
    file_watch: FileWatchRegistrationContext,
}

#[async_trait::async_trait]
impl SessionStartHookSideEffectSink for QuerySessionStartHookSink {
    async fn handle_session_start_hook_side_effects(&self, effects: SessionStartHookSideEffects) {
        if effects.watch_paths.is_empty() {
            return;
        }
        self.file_watch.add_paths(effects.watch_paths).await;
    }
}

impl SessionRuntime {
    /// Build a fresh `QueryEngine` for one turn using the runtime's
    /// stored `engine_config`. Both runners share this so the wiring
    /// can never drift. The session-memory text is refreshed from disk
    /// before each build so a fresh extraction shows up on the next turn.
    pub async fn build_engine(&self, cancel: CancellationToken) -> QueryEngine {
        self.build_engine_with_turn_abort(TurnAbortSignal::from_token(cancel))
            .await
    }

    pub async fn build_engine_with_turn_abort(&self, turn_abort: TurnAbortSignal) -> QueryEngine {
        self.build_engine_with_turn_abort_configured(turn_abort, |_| {})
            .await
    }

    pub async fn build_engine_with_turn_abort_configured<F>(
        &self,
        turn_abort: TurnAbortSignal,
        configure: F,
    ) -> QueryEngine
    where
        F: FnOnce(&mut QueryEngineConfig),
    {
        let mut engine_config = self.current_engine_config().await;
        configure(&mut engine_config);
        self.prepare_live_permission_overlay(&mut engine_config)
            .await;
        let engine = QueryEngine::new_with_turn_abort(
            engine_config,
            self.model_runtimes.clone(),
            self.tools.clone(),
            turn_abort,
            Some(self.hook_registry.clone()),
        );
        let engine = self
            .wire_engine(engine, None, EnginePersistenceMode::MainSession)
            .await;
        // Inject the in-prompt-shell Bash handle into the LIVE command registry
        // so skill / `ShellExpandingPromptHandler` markers route through the real
        // Bash tool with a per-command permission check. Refreshed on every
        // main-session engine build, so it also survives a `/reload-plugins`
        // registry swap (the new registry starts with an empty handle cell).
        let base_ctx = engine.build_base_tool_context().await;
        let registry = self.command_registry.read().await.clone();
        // One handle, two consumers: the command registry (slash /
        // shell-expanding prompt commands) and the skill runtime's shared
        // cell (model-invoked + fork-mode skills). Refreshed every build so
        // it survives a `/reload-plugins` registry swap and tracks the
        // latest tool config / cwd.
        let bash_handle = crate::bash_tool_handle::build_session_bash_handle(base_ctx);
        registry.set_bash_tool_handle(bash_handle.clone());
        if let Ok(mut cell) = self.skill_bash_cell.write() {
            *cell = Some(bash_handle);
        }
        // Late-bind the session id so user-typed skill slash commands can
        // substitute `${CLAUDE_SESSION_ID}`.
        registry.set_session_id(self.current_typed_session_id().await);
        // Capture this engine's cache-safe-params handle so a between-turns
        // `/btw` fork can read the parent turn's `CacheSafeParams` even though
        // the engine is rebuilt per turn. The handle is an `Arc` shared with
        // the engine's slot - it keeps observing writes the engine makes at
        // turn finalize and outlives the engine drop.
        *self.last_engine_cache_handle.write().await = Some(engine.cache_safe_params_handle());
        engine
    }

    pub async fn analyze_main_context(
        &self,
    ) -> coco_query::context_analysis::Result<coco_query::context_analysis::ContextUsageReport>
    {
        let history = {
            let guard = self.history.lock().await;
            guard.snapshot()
        };
        self.analyze_context_snapshot(history, None).await
    }

    pub async fn analyze_context_snapshot(
        &self,
        history: MessageHistory,
        app_state_override: Option<Arc<RwLock<ToolAppState>>>,
    ) -> coco_query::context_analysis::Result<coco_query::context_analysis::ContextUsageReport>
    {
        let engine = self.build_engine(CancellationToken::new()).await;
        let engine = if let Some(app_state) = app_state_override {
            engine.with_app_state(app_state)
        } else {
            engine
        };
        coco_query::context_analysis::analyze_engine_context_with_sources(
            &engine,
            &history,
            Some(self.skill_manager.clone()),
        )
        .await
    }

    /// Build a fresh `QueryEngine` from a caller-provided
    /// `QueryEngineConfig`. Used by SDK paths whose per-turn config
    /// fields (model, session_id, max_*) come from the
    /// `turn/start` request and override the runtime defaults.
    /// `app_state_override` lets the caller pin a specific
    /// `ToolAppState` Arc - SDK passes `Some(handoff.app_state)` so
    /// per-session app state and the compaction observers built from
    /// it stay coherent. TUI passes `None` and inherits
    /// `runtime.app_state`.
    pub async fn build_engine_from_config(
        &self,
        config: QueryEngineConfig,
        cancel: CancellationToken,
        app_state_override: Option<Arc<RwLock<ToolAppState>>>,
    ) -> QueryEngine {
        self.build_engine_from_config_with_persistence(
            config,
            cancel,
            app_state_override,
            EnginePersistenceMode::MainSession,
        )
        .await
    }

    /// Build a fork engine from a caller-provided config. Fork engines
    /// share runtime services but never write to the parent main-session
    /// transcript, usage tracker, or file-history sink.
    pub(crate) async fn build_fork_engine_from_config(
        &self,
        config: QueryEngineConfig,
        cancel: CancellationToken,
        app_state_override: Option<Arc<RwLock<ToolAppState>>>,
    ) -> QueryEngine {
        self.build_engine_from_config_with_persistence(
            config,
            cancel,
            app_state_override,
            EnginePersistenceMode::Fork,
        )
        .await
    }

    async fn build_engine_from_config_with_persistence(
        &self,
        config: QueryEngineConfig,
        cancel: CancellationToken,
        app_state_override: Option<Arc<RwLock<ToolAppState>>>,
        persistence: EnginePersistenceMode,
    ) -> QueryEngine {
        // Top-level SDK/headless session engines get the same live overlay as
        // the TUI main engine so a mid-cycle approval takes effect this cycle.
        // Gated to the main session (`MainSession` + no `agent_id`): subagents
        // and forks keep their own isolated config-cloned rules - they must not
        // share or reconcile the main session's overlay.
        // `config` is owned and local - the `mut` is confined to this block so
        // the rest of the function sees an immutable binding. The only shared
        // state touched is the overlay Arc, serialized by its `RwLock` inside
        // the helper; there is no cross-task sharing of `config` itself.
        let config = if matches!(persistence, EnginePersistenceMode::MainSession)
            && config.agent_id.is_none()
        {
            let mut config = config;
            self.prepare_live_permission_overlay(&mut config).await;
            config
        } else {
            config
        };
        // Fork isolation for the file-read dedup cache: when a fork sets
        // `clone_file_read_state` (default true for every framework fork),
        // give the child a *deep clone* of the parent's `FileReadState`
        // instead of the shared Arc `wire_engine` installs. The fork then
        // sees the parent's already-seen ids (cache parity preserved) but
        // its own reads/edits can't pollute the parent's cache.
        // `createSubagentContext`, which clones `readFileState` per fork.
        let isolate_file_read_state = config
            .fork_isolation
            .as_ref()
            .is_some_and(|iso| iso.clone_file_read_state);
        let engine = QueryEngine::new(
            config,
            self.model_runtimes.clone(),
            self.tools.clone(),
            cancel,
            Some(self.hook_registry.clone()),
        );
        let mut engine = self
            .wire_engine(engine, app_state_override, persistence)
            .await;
        if isolate_file_read_state {
            let snapshot = self.file_read_state.read().await.clone();
            engine = engine.with_file_read_state(Arc::new(RwLock::new(snapshot)));
        }
        engine
    }

    /// Install every per-session subsystem on a pre-built engine. The
    /// single source of truth for "what subsystems an engine needs" -
    /// both runners route through this so a new subsystem only needs
    /// adding here, not in two transport-specific spots.
    /// `app_state_override`: when `Some`, this Arc is what the engine
    /// gets via `with_app_state`, AND it's what the compaction
    /// observers reset on `notify_all`. When `None`, falls back to the
    /// runtime's own `app_state`. Without this override, SDK's
    /// `handoff.app_state` would be installed on the engine but
    /// `runtime.app_state` would be reset by observers - the two would
    /// drift after every compaction.
    pub(super) async fn wire_engine(
        &self,
        mut engine: QueryEngine,
        app_state_override: Option<Arc<RwLock<ToolAppState>>>,
        persistence: EnginePersistenceMode,
    ) -> QueryEngine {
        let app_state = app_state_override.unwrap_or_else(|| self.app_state.clone());
        engine = engine.with_file_read_state(self.file_read_state.clone());
        engine = engine.with_app_state(app_state.clone());
        // `auto_mode_state` is a SESSION-GLOBAL flag shared by every engine in
        // this runtime. Pure Auto still keys off the per-call
        // `permission_context.mode`; Plan uses this flag as the authoritative
        // plan-auto bridge signal, matching TS `mode === 'plan' &&
        // isAutoModeActive()`. Sync it from the session's authoritative
        // `self.app_state` (NOT the per-build `app_state` override): a
        // fork/skill/compaction sub-engine carrying a non-Auto override would
        // otherwise clobber it. Every build re-syncs from the single source,
        // covering all mode-change funnels (TUI + SDK) uniformly without
        // threading the flag through each.
        let engine_config = self.current_engine_config().await;
        {
            let mut app_state = self.app_state.write().await;
            let allow_rules = app_state.permissions.allow_rules.clone();
            let plan_auto_options = coco_permissions::PlanModeAutoOptions {
                use_auto_mode_during_plan: engine_config.use_auto_mode_during_plan,
                auto_mode_available: engine_config.permission_mode_availability.auto,
            };
            let _ = coco_permissions::reconcile_plan_auto_mode_in_app_state(
                &mut app_state,
                &allow_rules,
                plan_auto_options,
                &self.auto_mode_state,
            );
            if app_state.permissions.mode != Some(coco_types::PermissionMode::Plan) {
                self.auto_mode_state.set_active(
                    app_state.permissions.mode == Some(coco_types::PermissionMode::Auto),
                );
            }
        }
        // Build the classifier rules from settings (`auto_mode` is restricted
        // to user/policy sources by the per-source validator). Previously this
        // passed `::default()`, so allow/soft_deny/environment AND the
        // classifier mode were all silently dropped.
        let auto_mode_rules = self
            .runtime_config
            .settings
            .merged
            .auto_mode
            .as_ref()
            .map(|c| coco_permissions::AutoModeRules {
                allow: c.allow.clone(),
                soft_deny: c.soft_deny.clone(),
                environment: c.environment.clone(),
                classifier_mode: c.classifier_mode,
                classifier_unavailable_fail_open: c.classifier_unavailable_fail_open,
                classify_all_shell: self
                    .runtime_config
                    .settings
                    .auto_mode_classify_all_shell_enabled(),
            })
            .unwrap_or_default();
        engine = engine.with_auto_mode(
            self.auto_mode_state.clone(),
            self.denial_tracker.clone(),
            auto_mode_rules,
        );
        // Skill-emitted `permission_updates` now flow through the
        // engine's own per-engine `EngineLiveRulesHandle`
        // (auto-installed by `QueryEngine::new`) which writes into
        // `QueryEngine.live_command_rules` - a fresh Arc per engine
        // = per user message. No session-level handle install: that
        // would leak rules across user messages. See `engine_live_rules`
        // for the lifecycle invariant.
        // Session-scoped steering primitive. Without this, a fresh
        // `CommandQueue::new()` is constructed in `QueryEngine::new` and
        // dies with the per-turn engine, so any producer (TUI bridge,
        // future task / coordinator forwarders) enqueueing on
        // `runtime.command_queue()` would land on an instance the
        // running engine cannot see.
        engine = engine.with_command_queue(self.command_queue.clone());
        // Same lifetime argument as `with_command_queue`: the attachment
        // channel must live across engine rebuilds so cross-turn
        // producers (TUI slash commands, future swarm forwarders) see a
        // stable handle. The engine's own per-instance attachment
        // channel is replaced by the session-scoped one.
        engine = engine.with_attachment_channel(
            self.session_attachment_tx.clone(),
            self.session_attachment_rx.clone(),
        );
        if let Some(runtime) = &self.memory_runtime {
            let svc = runtime.session_memory.clone();
            let sm_text_now = svc.current_text().await;
            engine = engine.with_session_memory_text(sm_text_now);
            engine = engine.with_session_memory_service(svc);
        }
        // Install the real swarm-backed AgentHandle so AgentTool /
        // SendMessageTool reach the swarm runtime on every engine instance.
        engine = engine.with_agent_handle(self.swarm_agent_handle.clone());
        // Install the per-engine sync-hook-event buffer so the
        // `OrchestrationContext.sync_event_sink` constructed from this
        // engine's `orchestration_ctx()` writes into the same buffer
        // that the reminder source below drains.
        engine = engine.with_sync_hook_buffer(self.sync_hook_buffer.clone());
        // Same wiring for async hooks: the engine's `orchestration_ctx`
        // populates `async_registry` so engine-fired async hooks
        // (PreToolUse / PostToolUse / Stop / SubagentStop with
        // `is_async: true`) deliver via `CombinedHookEventsSource`.
        engine = engine.with_async_hook_registry(self.async_hook_registry.clone());
        // Same wiring for the LLM-driven hook handler so the engine's
        // `orchestration_ctx` carries it on every fired event. Usage
        // recording is scoped per engine; the shared handle only owns model
        // runtime state and the late-bound HookAgent runner.
        engine = engine.with_hook_llm_handle(Arc::new(
            self.hook_llm_handle
                .scoped_with_usage_accounting(self.usage_accounting.clone()),
        ));
        engine = engine.with_model_runtimes(self.model_runtimes.clone());
        engine =
            engine.with_session_start_hook_side_effect_sink(Arc::new(QuerySessionStartHookSink {
                file_watch: self.file_watch_registration_context(),
            }));
        if let Some(runtime) = &self.memory_runtime {
            engine = engine.with_memory_runtime(runtime.clone());
        }
        if let Some(rt) = &self.skill_review_runtime {
            engine = engine.with_skill_review_runtime(rt.clone());
        }
        // Reminder sources - populated unconditionally so non-memory
        // sessions still get hook + skill reminders. Each slot is
        // optional and silently skips if its data is empty.
        let team_snapshot = self.resolve_team_snapshot().await;
        let task_runtime = self.current_task_runtime().await;
        let sources = coco_system_reminder::ReminderSources {
            // Combined hook source: async-hook registry drains first,
            // then the sync-hook buffer that orchestration just wrote.
            hook_events: Some(Arc::new(
                coco_hooks::reminder_source::CombinedHookEventsSource::new(
                    self.async_hook_registry.clone(),
                    self.sync_hook_buffer.clone(),
                ),
            )),
            // Memory source: only when the runtime is built (gated on
            // `Feature::AutoMemory` upstream).
            memory: self.memory_runtime.as_ref().map(|runtime| {
                Arc::new(coco_query::reminder_adapters::MemoryAdapter::new(
                    runtime.clone(),
                )) as Arc<dyn coco_system_reminder::MemorySource>
            }),
            // Skills source: in-process `SkillManager` Arc kept alive
            // for the session. Empty manager => generator short-circuits.
            skills: Some(self.skill_manager.clone() as Arc<dyn coco_system_reminder::SkillsSource>),
            // Running-task source: TaskRuntime owns both the TaskManager row
            // state and the disk output reader needed for offset-based
            // task_status bookkeeping.
            task_status: task_runtime
                .map(|rt| rt as Arc<dyn coco_system_reminder::TaskStatusSource>),
            // Swarm source: drains peer messages from the shared pending
            // store, so a teammate's `SendMessage` surfaces as an
            // `agent_pending_messages` reminder on the recipient's next turn.
            // MUST share the SAME `Arc` as `engine.with_pending_messages`
            // below (the producer side) - otherwise messages vanish.
            swarm: Some(Arc::new(
                coco_query::reminder_adapters::SwarmAdapter::new()
                    .with_pending_messages(self.pending_message_store.clone())
                    .with_team_context(team_snapshot),
            ) as Arc<dyn coco_system_reminder::SwarmSource>),
            ..Default::default()
        };
        engine = engine.with_reminder_sources(sources);
        // Producer side of the pending-message pipeline: `SendMessage` pushes
        // into `ToolUseContext.pending_messages` (= this store). Shared across
        // the leader + in-process teammate engines (both via `wire_engine`).
        engine = engine.with_pending_messages(self.pending_message_store.clone());
        // Build observers fresh per call so the FileReadState and
        // AppState observers reference the engine's actual handles.
        // Cheap - the registry is just a Vec of Arc<dyn Observer>.
        let observers = coco_query::observers::build_default_registry(
            Some(self.file_read_state.clone()),
            Some(self.denial_tracker.clone()),
            Some(app_state),
            Some(self.loop_sentinel_state.clone()),
        );
        engine = engine.with_compaction_observers(observers);
        engine = engine.with_mailbox(self.mailbox.clone());
        // Install the MCP handle so AgentTool::prompt's per-turn
        // dynamic listing can pre-filter agents whose
        // `required_mcp_servers` aren't connected. Snapshot semantics:
        // each engine instance reads the handle slot at wire time;
        // hot-reloads land on the next engine.
        if let Some(mcp) = self.mcp_handle.read().await.clone() {
            engine = engine.with_mcp_handle(mcp);
        }
        engine = engine.with_schedule_store(self.schedule_store.clone());
        // Same snapshot pattern as MCP - every per-turn engine reads
        // the late-bound LSP slot once at wire time. Hot-reloads of
        // the LSP config land on the next engine build.
        if let Some(lsp) = self.lsp_handle.read().await.clone() {
            engine = engine.with_lsp_handle(lsp);
        }
        // Install the agent catalog snapshot so `AgentTool::prompt`
        // renders the dynamic per-turn agent listing. Without this the
        // engine falls back to `AgentTool`'s static description and
        // the model never sees the agents it can actually spawn.
        // Each engine instance captures the inner `Arc<...>` once at
        // wire time; concurrent `/agents reload` swaps land on the
        // next per-turn engine, not the in-flight one.
        engine = engine.with_agent_catalog(self.agent_catalog.read().await.clone());
        // config_home drives plan-mode (`plans_dir` / `session_plan_file`)
        // independent of persistence - always wire it; only the file-history
        // snapshot store is gated by persistence.
        engine = engine.with_config_home(self.config_home.clone());
        if persistence == EnginePersistenceMode::MainSession
            && self.persist_session
            && let Some(fh) = &self.file_history
        {
            engine = engine.with_file_history(fh.clone(), self.config_home.clone());
        }
        if let Some(bridge) = &self.permission_bridge {
            engine = engine.with_permission_bridge(bridge.clone());
        }
        // Main-session transcript persistence. Same `TranscriptStore`
        // instance feeds both the per-turn user / assistant JSONL
        // append in `engine_finalize_turn::record_transcript_tail`
        // and the metadata writes already wired there. The dedup set
        // lives on `SessionRuntime` so a fresh per-turn engine doesn't
        // re-write history each time; writes are keyed by session id and
        // skip already-persisted uuids.
        if persistence == EnginePersistenceMode::MainSession && self.persist_session {
            let transcript_session_id = self.current_typed_session_id().await;
            engine =
                engine.with_transcript_store(self.transcript_store.clone(), transcript_session_id);
            engine = engine.with_usage_accounting(self.usage_accounting.clone());
            engine = engine.with_transcript_dedup(self.transcript_dedup.clone());
            engine = engine
                .with_terminal_goal_metadata_flag(self.terminal_goal_metadata_written.clone());
            engine = engine
                .with_tool_result_replacement_state(self.tool_result_replacement_state.clone());
        }
        // Agent handle: installed by bootstrap after TaskRuntime exists.
        // Until then the engine carries the explicit no-op handle from
        // `swarm_agent_handle`.
        if let Some(handle) = self.agent_handle.read().await.clone() {
            engine = engine.with_agent_handle(handle);
        }
        // Skill handle: installed by bootstrap (`agent_handle_factory`)
        // once the subagent engine adapter exists. Until then the engine
        // carries `NoOpSkillHandle` and every `SkillTool` call returns
        // `Unavailable`. Installed on subagent engines too (this runs via
        // `build_engine_from_config`) so children can invoke skills.
        if let Some(handle) = self.skill_handle.read().await.clone() {
            engine = engine.with_skill_handle(handle);
        }
        // Fork dispatcher (D1/D2). Same late-bind contract as
        // `agent_handle` - installed only when `attach_fork_dispatcher`
        // ran at bootstrap. Without it, post-turn forks fall back to
        // their no-op paths (placeholder text / silent skip).
        if let Some(dispatcher) = self.fork_dispatcher.read().await.clone() {
            engine = engine.with_fork_dispatcher(dispatcher);
        }
        // Session-scoped prompt-suggestion abort slot. Sharing the same
        // `Arc` across every per-turn engine lets a new spawn cancel the
        // in-flight previous one.
        engine = engine.with_current_suggestion_abort(self.current_suggestion_abort.clone());
        // Production task runtime - same `Arc` is shared with
        // `SwarmAgentHandle` so AgentTool background spawns and the
        // engine's `Task*` tools see one source of truth.
        if let Some(rt) = self.task_runtime.read().await.clone() {
            engine = engine.with_task_handle(rt as coco_tool_runtime::BackgroundTaskHandleRef);
        }
        if let Some(task_list) = self.task_list.read().await.clone() {
            engine = engine.with_task_list(task_list);
        }
        if let Some(router) = self.team_task_list_router.read().await.clone() {
            engine = engine.with_team_task_list_router(router);
        }
        engine = engine.with_todo_list(self.todo_list.read().await.clone());
        engine
    }
}
