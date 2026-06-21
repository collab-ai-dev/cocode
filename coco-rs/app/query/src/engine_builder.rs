//! Builder + accessor impl for [`QueryEngine`].
//!
//! This file owns:
//! - the `new` constructor + every `with_*` builder,
//! - small one-liner accessors (`command_queue`, `inbox`),
//! - the recently-mentioned-paths LRU + post-compact-skill snapshot setters,
//! - the `attachment_emitter` handle and the matching drain helper,
//! - cache-break attribution + collapse-active gating.
//!
//! Extracted from `engine.rs` to keep that file focused on the multi-turn
//! orchestration. Layout follows `coco-rs` conventions: a sibling impl block
//! over the same struct, no extra module type. Field access works because
//! `QueryEngine` fields are `pub(crate)`.

use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use coco_context::FileHistoryState;
use coco_hooks::HookRegistry;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::ModelRuntimeSource;
use coco_messages::CostTracker;
use coco_messages::MessageHistory;
use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::TurnAbortSignal;
use coco_types::ToolAppState;

use crate::command_queue::CommandQueue;
use crate::config::QueryEngineConfig;
use crate::config::SessionBootstrap;
use crate::engine::QueryEngine;

impl QueryEngine {
    /// Cap on the number of @mentioned paths kept for post-compact priority.
    /// Bounds memory and matches the observation that compact restoration
    /// only ever lists 5 files anyway — older mentions stop mattering.
    pub(crate) const MENTION_PRIORITY_CAPACITY: usize = 32;

    pub fn new(
        config: QueryEngineConfig,
        model_runtimes: Arc<ModelRuntimeRegistry>,
        tools: Arc<ToolRegistry>,
        cancel: CancellationToken,
        hooks: Option<Arc<HookRegistry>>,
    ) -> Self {
        Self::new_with_turn_abort(
            config,
            model_runtimes,
            tools,
            TurnAbortSignal::from_token(cancel),
            hooks,
        )
    }

    pub fn new_with_turn_abort(
        config: QueryEngineConfig,
        model_runtimes: Arc<ModelRuntimeRegistry>,
        tools: Arc<ToolRegistry>,
        turn_abort: TurnAbortSignal,
        hooks: Option<Arc<HookRegistry>>,
    ) -> Self {
        let cancel = turn_abort.token();
        let (attachment_tx, attachment_rx) = tokio::sync::mpsc::unbounded_channel();
        // Per-engine (= per-user-msg) skill-rule store + matching handle.
        // Shared via Arc with the factory's batch-time merge and with
        // the handle the executor uses to fold tool-emitted updates.
        // See `engine_live_rules` for lifetime + subagent-isolation
        // rationale.
        // Pre-seed per-engine isolated Command-source rules a subagent/skill
        // carries (config.initial_command_rules). Empty for the main session.
        let live_command_rules: Arc<RwLock<Vec<coco_types::PermissionRule>>> =
            Arc::new(RwLock::new(config.initial_command_rules.clone()));
        let permission_rule_handle: coco_tool_runtime::PermissionRuleHandleRef = Arc::new(
            crate::engine_live_rules::EngineLiveRulesHandle::new(live_command_rules.clone()),
        );
        Self {
            config,
            tools,
            cancel,
            turn_abort,
            hooks,
            async_hook_registry: None,
            hook_llm_handle: None,
            model_runtimes,
            model_runtime_source: ModelRuntimeSource::Role(coco_types::ModelRole::Main),
            pending_tool_use_summary: Arc::new(tokio::sync::Mutex::new(None)),
            command_queue: CommandQueue::new(),
            file_read_state: None,
            file_history: None,
            config_home: None,
            session_bootstrap: None,
            permission_bridge: None,
            auto_mode_state: None,
            denial_tracker: None,
            auto_mode_rules: coco_permissions::AutoModeRules::default(),
            app_state: None,
            mailbox: None,
            pending_messages: None,
            mcp_handle: None,
            schedule_store: None,
            lsp_handle: None,
            agent_handle: None,
            skill_handle: None,
            live_command_rules,
            permission_rule_handle,
            agent_catalog: None,
            last_cache_safe_params: Arc::new(tokio::sync::RwLock::new(None)),
            fork_dispatcher: None,
            current_suggestion_abort: None,
            task_handle: None,
            tool_result_replacement_state: Arc::new(tokio::sync::RwLock::new(
                coco_tool_runtime::tool_result_storage::ContentReplacementState::new(i64::MAX),
            )),
            task_list: None,
            team_task_list_router: None,
            todo_list: None,
            reminder_sources: coco_system_reminder::ReminderSources::default(),
            attachment_tx,
            attachment_rx: Arc::new(tokio::sync::Mutex::new(attachment_rx)),
            compaction_observers: Arc::new(coco_compact::CompactionObserverRegistry::new()),
            last_assistant_ms: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            last_summarized_message_id: Arc::new(std::sync::Mutex::new(None)),
            session_memory_text: Arc::new(tokio::sync::RwLock::new(String::new())),
            session_memory_service: None,
            memory_runtime: None,
            reactive_state: Arc::new(tokio::sync::Mutex::new(
                coco_compact::ReactiveCompactState::new(),
            )),
            auto_compact_state: Arc::new(tokio::sync::Mutex::new(
                coco_compact::ReactiveCompactState::new(),
            )),
            running_tasks: None,
            last_compact_state: Arc::new(std::sync::Mutex::new(None)),
            post_compact_skills: Arc::new(std::sync::RwLock::new(Vec::new())),
            session_start_hook_side_effect_sink: None,
            staged_ledger: None,
            staged_session_id: uuid::Uuid::new_v4(),
            recently_mentioned_paths: Arc::new(tokio::sync::RwLock::new(
                std::collections::VecDeque::new(),
            )),
            pending_reactive_context_management: Arc::new(tokio::sync::Mutex::new(None)),
            pending_just_compacted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            transcript_store: None,
            session_usage_tracker: None,
            session_usage_write_lock: None,
            transcript_session_id: None,
            transcript_dedup: None,
            live_transcript: None,
            pending_nested_memory: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            loaded_nested_memory_paths: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
            sync_hook_buffer: None,
        }
    }

    /// Install a shared sync-hook-event buffer. The same buffer instance
    /// must back the `CombinedHookEventsSource` registered via
    /// [`Self::with_reminder_sources`], so SessionStart /
    /// UserPromptSubmit hook output pushed during this turn drains into
    /// the per-turn reminder pipeline.
    pub fn with_sync_hook_buffer(mut self, buf: coco_hooks::SyncHookEventBuffer) -> Self {
        self.sync_hook_buffer = Some(buf);
        self
    }

    /// Install the shared `AsyncHookRegistry` so engine-fired async
    /// hooks (PreToolUse / PostToolUse / Stop / SubagentStop with
    /// `is_async: true`) deliver their output through the same
    /// reminder-pipeline channel as session-runtime hooks.
    pub fn with_async_hook_registry(
        mut self,
        registry: Arc<coco_hooks::async_registry::AsyncHookRegistry>,
    ) -> Self {
        self.async_hook_registry = Some(registry);
        self
    }

    /// Install the LLM-driven hook handler so `Prompt` / `Agent` hook
    /// handlers route through the runtime registry instead of falling back to
    /// passthrough text. `SessionRuntime` builds one
    /// `Arc<dyn HookLlmHandle>` per session and clones it onto every
    /// engine via this method.
    pub fn with_hook_llm_handle(mut self, handle: Arc<dyn coco_hooks::HookLlmHandle>) -> Self {
        self.hook_llm_handle = Some(handle);
        self
    }

    /// Install the shared model runtime registry for helper LLM calls.
    pub fn with_model_runtimes(mut self, registry: Arc<ModelRuntimeRegistry>) -> Self {
        self.model_runtimes = registry;
        self
    }

    /// Select the runtime source used by this engine's agent loop.
    pub fn with_model_runtime_source(mut self, source: ModelRuntimeSource) -> Self {
        self.model_runtime_source = source;
        self
    }

    pub(crate) fn runtime_snapshot(&self) -> Option<coco_inference::ModelRuntimeSnapshot> {
        self.model_runtimes
            .snapshot_for_source(self.model_runtime_source.clone())
            .ok()
    }

    pub(crate) async fn notify_model_compaction(&self, query_source: &str) {
        if let Ok(runtime) = self
            .model_runtimes
            .runtime_for_source(self.model_runtime_source.clone())
        {
            coco_inference::ModelRuntime::notify_active_compaction(
                runtime,
                query_source,
                self.config.agent_id.as_deref(),
            )
            .await;
        }
    }

    pub(crate) async fn notify_model_cache_deletion(&self, query_source: &str) {
        if let Ok(runtime) = self
            .model_runtimes
            .runtime_for_source(self.model_runtime_source.clone())
        {
            coco_inference::ModelRuntime::notify_active_cache_deletion(
                runtime,
                query_source,
                self.config.agent_id.as_deref(),
            )
            .await;
        }
    }

    /// Install a transcript store for marble-origami persistence and
    /// per-turn user/assistant JSONL writes. `session_id` keys the
    /// transcript path; absent it the engine writes to a fresh
    /// in-memory ledger only (commits are lost on restart).
    pub fn with_transcript_store(
        mut self,
        store: Arc<dyn coco_session::SessionStore>,
        session_id: String,
    ) -> Self {
        self.transcript_store = Some(store);
        self.transcript_session_id = Some(session_id);
        self
    }

    pub fn with_session_usage_tracker(
        mut self,
        tracker: Arc<tokio::sync::Mutex<CostTracker>>,
    ) -> Self {
        self.session_usage_tracker = Some(tracker);
        if self.session_usage_write_lock.is_none() {
            self.session_usage_write_lock = Some(Arc::new(tokio::sync::Mutex::new(())));
        }
        self
    }

    pub fn with_session_usage_write_lock(mut self, lock: Arc<tokio::sync::Mutex<()>>) -> Self {
        self.session_usage_write_lock = Some(lock);
        self
    }

    pub(crate) async fn record_session_usage(
        &self,
        event_tx: &Option<tokio::sync::mpsc::Sender<crate::CoreEvent>>,
        provider: &str,
        model_id: &str,
        usage: coco_types::TokenUsage,
        duration_ms: i64,
    ) {
        let Some(tracker) = &self.session_usage_tracker else {
            return;
        };
        let _write_guard = match &self.session_usage_write_lock {
            Some(lock) => Some(lock.lock().await),
            None => None,
        };
        let snapshot = {
            let mut guard = tracker.lock().await;
            guard.record_usage(provider, model_id, usage, duration_ms);
            guard.snapshot(&self.config.session_id)
        };
        let _ = crate::emit::emit_protocol(
            event_tx,
            crate::ServerNotification::SessionUsageUpdated(Box::new(snapshot.clone())),
        )
        .await;
        if let Some(store) = &self.transcript_store {
            let store = Arc::clone(store);
            let session_id = self.config.session_id.clone();
            let snapshot = snapshot.clone();
            match tokio::task::spawn_blocking(move || {
                store.write_usage_snapshot(&session_id, &snapshot)
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(
                        error = %e,
                        session_id = %self.config.session_id,
                        "failed to write session usage snapshot"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session_id = %self.config.session_id,
                        "usage snapshot write task failed"
                    );
                }
            }
        }
    }

    pub fn with_tool_result_replacement_state(
        mut self,
        state: coco_tool_runtime::tool_result_storage::ContentReplacementStateRef,
    ) -> Self {
        self.tool_result_replacement_state = state;
        self
    }

    /// Install the cross-engine dedup set for transcript message
    /// writes. `SessionRuntime` owns one `Arc<Mutex<HashSet<Uuid>>>`
    /// per session and clones it into every per-turn engine so
    /// already-persisted messages don't get rewritten on each new
    /// engine instance. Skips already-persisted entries by uuid.
    pub fn with_transcript_dedup(
        mut self,
        seen: Arc<tokio::sync::Mutex<std::collections::HashSet<uuid::Uuid>>>,
    ) -> Self {
        self.transcript_dedup = Some(seen);
        self
    }

    /// Install a staged-collapse ledger. Required for the
    /// `experimental.staged_compact` strategy; absent by default. The
    /// ledger is shared so resume / persistence layers can read commits.
    pub fn with_staged_ledger(
        mut self,
        ledger: Arc<tokio::sync::Mutex<coco_compact::StagedCompactLedger>>,
        session_id: uuid::Uuid,
    ) -> Self {
        self.staged_ledger = Some(ledger);
        self.staged_session_id = session_id;
        self
    }

    /// Whether the staged-collapse strategy is currently active. Used
    /// as the mutual-exclusion gate for autocompact — when collapse
    /// owns the threshold ladder, proactive autocompact is suppressed.
    ///
    /// Inert by default: `with_staged_ledger` has no production callers
    /// (matches TS-feature-stripped `feature('CONTEXT_COLLAPSE')` state),
    /// so the first AND-clause is always false and this returns `false`.
    /// Wire `with_staged_ledger` and flip
    /// `compact.experimental.staged_compact.enabled` together to opt in.
    pub fn is_collapse_active(&self) -> bool {
        self.staged_ledger.is_some() && self.config.compact.experimental.staged_compact.enabled
    }

    // ── Post-turn cache-safe params (D8) ──

    /// Snapshot the current post-turn cache-safe params. `None` until
    /// the first turn finalises. Post-turn fork features
    /// (`promptSuggestion`, compaction, side-query) read this to share
    /// the parent's prompt cache.
    pub async fn last_cache_safe_params(&self) -> Option<coco_types::CacheSafeParams> {
        self.last_cache_safe_params.read().await.clone()
    }

    /// Clone the `Arc<RwLock<...>>` so observers (the TUI runtime, transcript
    /// recorder) can poll the slot across the per-turn engine's lifetime
    /// without holding a `&QueryEngine`. Read-only contract: external holders
    /// observe the slot; the engine is the only writer (via
    /// `save_cache_safe_params` at turn finalize). `/clear` drops the handle
    /// at the runtime layer rather than mutating the inner slot.
    pub fn cache_safe_params_handle(
        &self,
    ) -> std::sync::Arc<tokio::sync::RwLock<Option<coco_types::CacheSafeParams>>> {
        self.last_cache_safe_params.clone()
    }

    /// Engine-internal writer for the cache-safe params slot. Called
    /// from `finalize_turn_post_tools` after each successful turn —
    /// **not** part of the public API.
    pub(crate) async fn save_cache_safe_params(&self, params: coco_types::CacheSafeParams) {
        *self.last_cache_safe_params.write().await = Some(params);
    }

    /// Build a [`coco_types::CacheSafeParams`] from the current turn's
    /// post-execution history and save it. Called from every exit
    /// path of `run_session_loop` — both the tool-execution path
    /// (via `finalize_turn_post_tools`) and the text-only end-turn
    /// path that returns early.
    pub(crate) async fn save_post_turn_cache_params(
        &self,
        history: &coco_messages::MessageHistory,
    ) {
        if history.is_empty() {
            return;
        }
        // Snapshot the post-turn history into shared `Arc<Message>`
        // entries so the slot can be observed without holding a
        // parent-history reference. Same shape that
        // `AgentQueryConfig.fork_context_messages` expects, so a fork
        // caller threads it directly through the existing
        // fork-context plumbing — no serialize / deserialize hop.
        let fork_messages: Vec<std::sync::Arc<coco_messages::Message>> =
            history.as_slice().to_vec();
        let rendered_system_prompt = self.config.system_prompt.clone().unwrap_or_default();
        let provider = self
            .runtime_snapshot()
            .map(|snapshot| snapshot.provider)
            .unwrap_or_default();
        self.save_cache_safe_params(coco_types::CacheSafeParams {
            rendered_system_prompt,
            model_id: self.config.model_id.clone(),
            provider,
            active_shell_tool: self.config.active_shell_tool,
            prompt_cache: self.config.prompt_cache.clone(),
            fork_context_messages: fork_messages,
        })
        .await;
    }

    /// Cache-break tracking attribution. Subagents land under
    /// `agent:custom` (with their `agent_id`), SDK calls under `sdk`,
    /// everything else under `repl_main_thread`.
    ///
    /// When the engine config carries a `query_source_override`
    /// (set by [`crate::forked_agent::ForkDispatcher`] from
    /// [`crate::forked_agent::ForkedAgentOptions::query_source`]),
    /// the override wins so log lines self-identify the fork variant
    /// (e.g. `prompt_suggestion`, `extract_memories`,
    /// `session_memory_auto`) instead of collapsing all forks to `sdk`.
    pub(crate) fn query_source_label(&self) -> &str {
        if let Some(override_label) = self.config.query_source_override.as_deref() {
            return override_label;
        }
        if self.config.agent_id.is_some() {
            "agent:custom"
        } else if self.config.is_non_interactive {
            "sdk"
        } else {
            "repl_main_thread"
        }
    }

    /// Record paths that were just @mentioned so they get priority during
    /// post-compact file restoration. Newest entries push older ones out
    /// once the LRU exceeds [`Self::MENTION_PRIORITY_CAPACITY`].
    pub async fn note_mentioned_paths<I>(&self, paths: I)
    where
        I: IntoIterator<Item = std::path::PathBuf>,
    {
        let mut g = self.recently_mentioned_paths.write().await;
        for path in paths {
            // Move existing entry to the back instead of duplicating.
            if let Some(idx) = g.iter().position(|p| p == &path) {
                g.remove(idx);
            }
            g.push_back(path);
        }
        while g.len() > Self::MENTION_PRIORITY_CAPACITY {
            g.pop_front();
        }
    }

    /// Snapshot the current set of recently @mentioned paths.
    pub async fn recently_mentioned_paths_snapshot(
        &self,
    ) -> std::collections::HashSet<std::path::PathBuf> {
        let g = self.recently_mentioned_paths.read().await;
        g.iter().cloned().collect()
    }

    /// Replace the post-compact skill snapshot. Callers pre-convert from
    /// their canonical skill state (e.g.
    /// `coco_system_reminder::InvokedSkillEntry`) into
    /// `coco_compact::PostCompactSkill`. Empty vec disables the in-band
    /// path; the next-turn `InvokedSkillsGenerator` still re-injects.
    pub fn set_post_compact_skills(&self, skills: Vec<coco_compact::PostCompactSkill>) {
        if let Ok(mut g) = self.post_compact_skills.write() {
            *g = skills;
        }
    }

    /// Install a runtime sink for SessionStart hook side effects emitted
    /// during compact context restoration.
    pub fn with_session_start_hook_side_effect_sink(
        mut self,
        sink: crate::session_start_hooks::SessionStartHookSideEffectSinkRef,
    ) -> Self {
        self.session_start_hook_side_effect_sink = Some(sink);
        self
    }

    /// Install the session-memory text snapshot. Callers (CLI/TUI/SDK)
    /// load `~/.coco/<session>/session-memory/summary.md` at startup
    /// and refresh it after every extraction. Empty string ≡ no SM
    /// available, the SM-first compact path becomes a pass-through.
    pub fn with_session_memory_text(self, text: String) -> Self {
        if let Ok(mut guard) = self.session_memory_text.try_write() {
            *guard = text;
        }
        self
    }

    /// Async setter used by the extraction pipeline to refresh SM text
    /// after a forked-agent extraction completes.
    pub async fn set_session_memory_text(&self, text: String) {
        let mut guard = self.session_memory_text.write().await;
        *guard = text;
    }

    /// Install the session-memory service so `try_session_memory_compact`
    /// can wait for any in-flight extraction and read the cached body.
    /// This is the same `Arc` held by [`coco_memory::MemoryRuntime::session_memory`] —
    /// the engine keeps a separate handle so the compact path doesn't
    /// have to hop through an `Option<MemoryRuntime>` indirection.
    pub fn with_session_memory_service(
        mut self,
        svc: Arc<coco_memory::SessionMemoryService>,
    ) -> Self {
        self.session_memory_service = Some(svc);
        self
    }

    /// Install the auto-memory runtime — extraction / dream / 9-section
    /// session memory / recall ranker. Set by the CLI / SDK bootstrap
    /// when `Feature::AutoMemory` is enabled. Without this the
    /// turn-end fan-out stays inert.
    pub fn with_memory_runtime(mut self, runtime: Arc<coco_memory::MemoryRuntime>) -> Self {
        self.memory_runtime = Some(runtime);
        self
    }

    /// Public accessor used by `/dream` and `/summary` slash-command
    /// dispatchers in the SDK / TUI runner to reach the runtime
    /// services. Returns `None` when `Feature::AutoMemory` is off.
    pub fn memory_runtime(&self) -> Option<&Arc<coco_memory::MemoryRuntime>> {
        self.memory_runtime.as_ref()
    }

    /// Install the running-task manager so post-compact attachments can
    /// snapshot active background agents and re-emit them as
    /// `task_status` reminders. Reads from the `TaskManager` exposed by
    /// `coco-tasks::running`. Optional — when absent, post-compact emits
    /// zero `task_status` attachments.
    pub fn with_running_tasks(mut self, running: Arc<coco_tasks::running::TaskManager>) -> Self {
        self.running_tasks = Some(running);
        self
    }

    /// Snapshot running async-agent tasks for post-compact attachment
    /// emission. Filters: skip the agent that owns this engine's
    /// `agent_id`, drop pending tasks (not yet meaningful), drop
    /// terminal tasks the `TaskOutput` tool already consumed
    /// (`BgAgentExtras.retrieved`).
    pub(crate) async fn snapshot_async_agents_for_post_compact(
        &self,
    ) -> Vec<coco_compact::AsyncAgentSnapshot> {
        let Some(tasks) = &self.running_tasks else {
            return Vec::new();
        };
        let listed = tasks.list().await;
        let self_agent = self.config.agent_id.as_deref();
        let mut out = Vec::with_capacity(listed.len());
        for t in listed {
            if !matches!(t.task_type(), coco_types::TaskType::BgAgent) {
                continue;
            }
            if matches!(t.status, coco_types::TaskStatus::Pending) {
                continue;
            }
            // Skip the engine's own agent — it's already part of the
            // visible conversation, not a peer that the model could
            // duplicate.
            if let Some(a) = self_agent
                && t.id.as_str() == a
            {
                continue;
            }
            // Once the `TaskOutput` tool serves a terminal agent's
            // output, the compact reminder stops re-announcing it.
            // Read through the typed `BgAgentExtras` variant on `TaskExtras`.
            if t.status.is_terminal() && t.bg_agent_extras().map(|e| e.retrieved).unwrap_or(false) {
                continue;
            }
            out.push(coco_compact::AsyncAgentSnapshot {
                task_id: t.id.clone(),
                status: task_status_to_ts_string(t.status),
                description: t.description,
                delta_summary: None,
                output_file_path: t.output_file.unwrap_or_default(),
            });
        }
        out
    }

    /// Stamp the most recent assistant timestamp. Called from
    /// `run_session_loop` immediately after `record_session_usage` on
    /// every Finish / PrematureClose turn (Finding **R11**). Drives:
    /// (a) `time_since_last_assistant_ms` on the next-turn `QueryParams`,
    /// (b) `coco_compact::evaluate_time_based_trigger` inside
    /// `finalize_turn_post_tools` (time-based microcompact gate).
    pub fn stamp_assistant_now(&self) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        self.last_assistant_ms
            .store(now_ms, std::sync::atomic::Ordering::Release);
    }

    /// Install the compaction observer registry. Caller builds the
    /// registry, registers per-subsystem observers, then hands an
    /// `Arc` to the engine so notifications fire in `try_full_compact`.
    /// Omitting this leaves an empty registry — equivalent to skipping
    /// `runPostCompactCleanup` when the corresponding caches don't exist.
    pub fn with_compaction_observers(
        mut self,
        observers: Arc<coco_compact::CompactionObserverRegistry>,
    ) -> Self {
        self.compaction_observers = observers;
        self
    }

    /// A clone-friendly emitter handle for owning crates (hooks /
    /// permissions / commands / core/tool-runtime / skills) so they can push
    /// `Message::Attachment` entries into this session's history without
    /// direct access to the engine. Drained once per outer-loop turn.
    pub fn attachment_emitter(&self) -> coco_messages::AttachmentEmitter {
        coco_messages::AttachmentEmitter::new(self.attachment_tx.clone())
    }

    /// Replace the engine's per-instance attachment channel with a
    /// session-scoped one supplied by the caller (typically `SessionRuntime`).
    ///
    /// Required for producers that live across engine rebuilds — the
    /// TUI's slash-command handler holds an
    /// [`coco_messages::AttachmentEmitter`] from before the turn started,
    /// and the engine must drain from the same channel that emitter wrote
    /// to. Without this, a slash-command-emitted attachment would land in
    /// a stale `attachment_rx` that no engine ever drains.
    pub fn with_attachment_channel(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<coco_messages::AttachmentMessage>,
        rx: Arc<
            tokio::sync::Mutex<
                tokio::sync::mpsc::UnboundedReceiver<coco_messages::AttachmentMessage>,
            >,
        >,
    ) -> Self {
        self.attachment_tx = tx;
        self.attachment_rx = rx;
        self
    }

    /// Drain any silent attachments emitted since the last turn into
    /// `history`. Called at the head of each outer-loop iteration.
    /// Returns the number of drained attachments for telemetry.
    pub(crate) async fn drain_attachment_inbox(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
    ) -> usize {
        let mut count = 0;
        let mut rx = self.attachment_rx.lock().await;
        while let Ok(att) = rx.try_recv() {
            // I-1 (Authority): every transcript-visible append must
            // emit so TUI's TranscriptView + SDK NDJSON observers
            // track the new attachment. Drain happens at turn-start
            // before the reminder pipeline so cross-crate-produced
            // attachments land in history with full provenance.
            crate::history_sync::history_push_and_emit(
                history,
                coco_messages::Message::Attachment(att),
                event_tx,
            )
            .await;
            count += 1;
        }
        count
    }

    /// Install the per-subsystem reminder source bundle. Each
    /// `Some(Arc<dyn XxxSource>)` field powers a category of
    /// system-reminders that needs state from an owning crate
    /// (hooks, LSP, tasks, skills, MCP, swarm, bridge, memory).
    /// Omitted sources → corresponding reminders silently skip.
    pub fn with_reminder_sources(mut self, sources: coco_system_reminder::ReminderSources) -> Self {
        self.reminder_sources = sources;
        self
    }

    /// Install a mailbox handle for swarm teammate messaging.
    pub fn with_mailbox(mut self, mailbox: coco_tool_runtime::MailboxHandleRef) -> Self {
        self.mailbox = Some(mailbox);
        self
    }

    /// Install the pending-message store so `SendMessage` to a running
    /// teammate queues the message and the recipient sees it via the
    /// `agent_pending_messages` system-reminder. Production wires the
    /// SAME `Arc<InMemoryPendingMessageStore>` here AND on the
    /// `SwarmAdapter`. Default is a no-op store.
    pub fn with_pending_messages(
        mut self,
        store: coco_tool_runtime::PendingMessageStoreRef,
    ) -> Self {
        self.pending_messages = Some(store);
        self
    }

    /// Install the MCP handle so prompt-rendering can read the
    /// connected-server set and pre-filter agents whose
    /// `required_mcp_servers` aren't ready. `None` (the default)
    /// makes the AgentTool prompt skip MCP filtering at the renderer.
    pub fn with_mcp_handle(mut self, handle: coco_tool_runtime::McpHandleRef) -> Self {
        self.mcp_handle = Some(handle);
        self
    }

    /// Install the scheduling backend for Cron*/RemoteTrigger tools.
    pub fn with_schedule_store(mut self, store: coco_tool_runtime::ScheduleStoreRef) -> Self {
        self.schedule_store = Some(store);
        self
    }

    /// Install the LSP handle so `LspTool::is_enabled` reports
    /// connected state correctly **and** `LspTool::execute` can dispatch
    /// JSON-RPC requests through the server manager. `None` (the
    /// default) makes the tool hide itself from the model's tool list.
    pub fn with_lsp_handle(mut self, handle: coco_tool_runtime::LspHandleRef) -> Self {
        self.lsp_handle = Some(handle);
        self
    }

    /// Install the real [`AgentHandle`](coco_tool_runtime::AgentHandle) so
    /// `AgentTool` invocations route to the swarm / subagent
    /// runtime. Without this the factory defaults to
    /// `NoOpAgentHandle` and every `AgentTool` call returns a clean
    /// "not available in this context" error — fine for tests, but
    /// CLI / SDK / TUI runners should install a real handle at
    /// bootstrap.
    pub fn with_agent_handle(mut self, handle: coco_tool_runtime::AgentHandleRef) -> Self {
        self.agent_handle = Some(handle);
        self
    }

    /// Install a fork dispatcher (D1/D2). Used by post-turn forks
    /// (`/btw`, `promptSuggestion`) to drive a fresh engine without
    /// mutating the parent. CLI / SDK runners install the same
    /// instance — usually backed by `SessionRuntime`.
    /// Install the session-scoped prompt-suggestion abort slot. Called
    /// from `wire_engine` so every per-turn engine sees the same
    /// `Arc<Mutex<Option<CancellationToken>>>` shared across the
    /// session — rapid `/clear` cycles cancel the prior in-flight
    /// suggestion fork.
    pub fn with_current_suggestion_abort(
        mut self,
        slot: std::sync::Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    ) -> Self {
        self.current_suggestion_abort = Some(slot);
        self
    }

    pub fn with_fork_dispatcher(
        mut self,
        dispatcher: crate::forked_agent::ForkDispatcherRef,
    ) -> Self {
        self.fork_dispatcher = Some(dispatcher);
        self
    }

    /// Read the engine's installed fork dispatcher. `None` until
    /// installed via [`Self::with_fork_dispatcher`].
    pub fn fork_dispatcher(&self) -> Option<crate::forked_agent::ForkDispatcherRef> {
        self.fork_dispatcher.clone()
    }

    /// Install the background task runtime. CLI bootstrap shares
    /// the same `TaskRuntime` Arc between the engine (read/control
    /// side via this builder) and `SwarmAgentHandle` (registration
    /// side). When absent, the engine threads `None` into
    /// `ToolUseContext.task_handle`, where the task tools surface a
    /// "no task runtime configured" error.
    pub fn with_task_handle(mut self, handle: coco_tool_runtime::BackgroundTaskHandleRef) -> Self {
        self.task_handle = Some(handle);
        self
    }

    /// Install the real [`SkillHandle`](coco_tool_runtime::SkillHandle) so
    /// `SkillTool` invocations route to the skill runtime (inline
    /// expansion or forked subagent). Without this the factory
    /// defaults to `NoOpSkillHandle` and every skill call returns
    /// `SkillInvocationError::Unavailable` — the runner surfaces
    /// that as a model-visible error.
    pub fn with_skill_handle(mut self, handle: coco_tool_runtime::SkillHandleRef) -> Self {
        self.skill_handle = Some(handle);
        self
    }

    /// Install the agent-definition catalog snapshot (T7). When set,
    /// AgentTool resolves `subagent_type` to a full `AgentDefinition`
    /// at the spawn boundary so the runner reads `definition.model` /
    /// `definition.model_role`. Without this the catalog is
    /// unavailable and AgentTool falls back to subagent_type→role
    /// mapping alone — same behaviour as before T7. Bootstrap should
    /// pass `coco_subagent::AgentDefinitionStore::snapshot()` here.
    pub fn with_agent_catalog(mut self, catalog: Arc<coco_subagent::AgentCatalogSnapshot>) -> Self {
        self.agent_catalog = Some(catalog);
        self
    }

    /// Install the durable task-list store (V2 task tools).
    pub fn with_task_list(mut self, handle: coco_tool_runtime::TaskListHandleRef) -> Self {
        self.task_list = Some(handle);
        self
    }

    pub fn with_team_task_list_router(
        mut self,
        router: coco_tool_runtime::TeamTaskListRouterRef,
    ) -> Self {
        self.team_task_list_router = Some(router);
        self
    }

    /// Install the ephemeral per-agent todo store (V1 TodoWrite).
    pub fn with_todo_list(mut self, handle: coco_tool_runtime::TodoListHandleRef) -> Self {
        self.todo_list = Some(handle);
        self
    }

    /// Attach auto-mode state + rules so `PermissionDecision::Ask` outcomes
    /// are first classified by the 2-stage LLM sidequery before falling back
    /// to interactive approval.
    pub fn with_auto_mode(
        mut self,
        state: Arc<coco_permissions::AutoModeState>,
        denial_tracker: Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>>,
        rules: coco_permissions::AutoModeRules,
    ) -> Self {
        self.auto_mode_state = Some(state);
        self.denial_tracker = Some(denial_tracker);
        self.auto_mode_rules = rules;
        self
    }

    /// Attach session bootstrap data to be emitted as `SessionStarted`
    /// before the first turn. Without this, the engine still runs normally
    /// but does not emit `SessionStarted` (backwards compatible for tests).
    pub fn with_session_bootstrap(mut self, bootstrap: SessionBootstrap) -> Self {
        self.session_bootstrap = Some(bootstrap);
        self
    }

    /// Attach a permission bridge so `PermissionDecision::Ask` outcomes
    /// are forwarded to an external authority (e.g. the SDK client via
    /// `SdkPermissionBridge`) instead of auto-allowing.
    pub fn with_permission_bridge(
        mut self,
        bridge: coco_tool_runtime::ToolPermissionBridgeRef,
    ) -> Self {
        self.permission_bridge = Some(bridge);
        self
    }

    /// Set file read state for @mention dedup and changed-file detection.
    pub fn with_file_read_state(
        mut self,
        file_read_state: Arc<RwLock<coco_context::FileReadState>>,
    ) -> Self {
        self.file_read_state = Some(file_read_state);
        self
    }

    /// Attach a shared `ToolAppState` for cross-component signalling.
    ///
    /// Tools read/write this via `ToolUseContext.app_state` — plan mode's
    /// exit flag, plan-file entry timestamp, and the live permission
    /// mode (`permission_mode`, `pre_plan_mode`, `stripped_dangerous_rules`)
    /// are carried here. Without this the engine runs normally but
    /// the plan-mode-exit reminder never fires and tool mode changes
    /// don't propagate across LLM iterations.
    ///
    /// **Bootstrap**: if `app_state.permission_mode` is `None` (fresh
    /// state), it's seeded from `self.config.permission_mode` so the
    /// first batch's [`ToolContextFactory::build`] sees a concrete mode. If
    /// already `Some(_)` (e.g. session resumed, prior-run state
    /// carried), the existing value is preserved — user + tool
    /// intent trumps config. `appState` is initialized-once at
    /// session-create and never re-seeded from config afterward.
    pub fn with_app_state(mut self, app_state: Arc<RwLock<ToolAppState>>) -> Self {
        // Bootstrap the live mode on first attach. This is a one-shot
        // write — subsequent runs that reuse the same app_state see
        // the preserved value rather than an overwrite.
        if let Ok(mut guard) = app_state.try_write()
            && guard.permissions.mode.is_none()
        {
            guard.permissions.mode = Some(self.config.permission_mode);
        }
        self.app_state = Some(app_state);
        self
    }

    /// Set file history state for checkpoint/rewind support.
    pub fn with_file_history(
        mut self,
        file_history: Arc<RwLock<FileHistoryState>>,
        config_home: std::path::PathBuf,
    ) -> Self {
        self.file_history = Some(file_history);
        self.config_home = Some(config_home);
        self
    }

    /// Set the config home directory. Used by plan-mode (`plans_dir`
    /// resolution) and surfaced on `ToolUseContext.config_home` so
    /// tools can locate the plan file on disk. `with_file_history`
    /// also sets this as a side-effect; use this builder when you
    /// need config_home without attaching a file-history state (e.g.
    /// integration tests).
    pub fn with_config_home(mut self, config_home: std::path::PathBuf) -> Self {
        self.config_home = Some(config_home);
        self
    }

    /// Access the command queue for mid-turn steering. Producers
    /// (TUI bridge, future task / coordinator forwarders) call
    /// `enqueue` on this handle to inject messages between LLM
    /// iterations; the engine drains end-of-turn into the message
    /// history.
    pub fn command_queue(&self) -> &CommandQueue {
        &self.command_queue
    }

    /// Inject a session-scoped [`CommandQueue`].
    ///
    /// `QueryEngine` is built fresh per turn (`SessionRuntime::build_engine`),
    /// but the steering queue must survive across engines so messages typed
    /// during turn N are still pending when turn N+1's engine starts.
    /// The session runtime owns the queue and hands it to every engine
    /// via this builder.
    ///
    /// Teammate messages also flow through this queue (with
    /// [`coco_system_reminder::QueueOrigin::Coordinator`] /
    /// [`coco_system_reminder::QueueOrigin::TaskNotification`]) — coordinator
    /// messages become `queued_command` attachments with `origin: 'coordinator'`.
    pub fn with_command_queue(mut self, queue: CommandQueue) -> Self {
        self.command_queue = queue;
        self
    }

    /// Attach the live message-history sink read by the periodic
    /// AgentSummary timer. Installed on (background) sub-agent engines by
    /// `coco_query::agent_adapter` when the spawn enabled summarization;
    /// the engine then publishes a full post-turn snapshot from
    /// `record_transcript_tail`. The main loop never sets this.
    pub fn with_live_transcript(mut self, sink: coco_tool_runtime::LiveTranscript) -> Self {
        self.live_transcript = Some(sink);
        self
    }

    /// Override the engine's default
    /// [`PermissionRuleHandle`](coco_tool_runtime::PermissionRuleHandle).
    ///
    /// By default `QueryEngine::new` installs an
    /// [`crate::engine_live_rules::EngineLiveRulesHandle`] that folds
    /// `Command`-destination `permission_updates` into the engine's own
    /// `live_command_rules` Arc — the per-user-msg-scoped store for
    /// always-allowed command rules.
    /// Production paths inherit this default; tests/standalone callers
    /// can swap in [`coco_tool_runtime::NoOpPermissionRuleHandle`] when
    /// they want updates dropped instead.
    ///
    /// **Note:** overriding does NOT change [`QueryEngine.live_command_rules`].
    /// The factory still reads the original Arc for the batch-time
    /// merge — only the WRITE path is redirected. Use this to isolate
    /// writes in tests, not to reroute reads.
    pub fn with_permission_rule_handle(
        mut self,
        handle: coco_tool_runtime::PermissionRuleHandleRef,
    ) -> Self {
        self.permission_rule_handle = handle;
        self
    }
}

/// Render a `TaskStatus` to its wire string form —
/// `'pending' | 'running' | 'completed' | 'failed' | 'killed'`.
fn task_status_to_ts_string(status: coco_types::TaskStatus) -> String {
    match status {
        coco_types::TaskStatus::Pending => "pending",
        coco_types::TaskStatus::Running => "running",
        coco_types::TaskStatus::Completed => "completed",
        coco_types::TaskStatus::Failed => "failed",
        coco_types::TaskStatus::Killed => "killed",
    }
    .to_string()
}
