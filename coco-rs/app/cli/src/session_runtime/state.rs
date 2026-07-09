use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tracing::warn;

use coco_messages::Message;
use coco_query::CommandQueue;
use coco_query::QueryEngineConfig;
use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::ToolRegistry;
use coco_types::ProviderModelSelection;
use coco_types::SessionId;
use tokio_util::sync::CancellationToken;

use super::SessionRuntime;
use super::clone_std_rwlock;
use super::hooks::async_rewake_sink;
use super::resolve_model_selection_from_runtime_config;
use super::write_std_rwlock;

mod file_history;

pub(super) use file_history::TranscriptFileHistorySink;
pub(super) use file_history::file_checkpointing_enabled;

impl SessionRuntime {
    /// Session-scoped attachment emitter for producers outside the
    /// per-turn engine (TUI slash commands, swarm forwarders, ...).
    /// Each `emit()` enqueues a typed `AttachmentMessage` (typically
    /// silent-* variants) onto the session channel. The engine drains
    /// at the head of each outer-loop turn via
    /// [`coco_query::QueryEngine::drain_attachment_inbox`] so producers
    /// don't need access to `MessageHistory`.
    pub fn attachment_emitter(&self) -> coco_messages::AttachmentEmitter {
        self.command_resources.attachment_emitter()
    }

    /// The tool registry shared by every engine instance.
    /// Callers that need to register or deregister tools at runtime (e.g.
    /// the SDK MCP lifecycle handlers) use this to mutate the registry
    /// via its interior-mutability API.
    pub fn tools(&self) -> &Arc<ToolRegistry> {
        self.execution.tools()
    }

    /// Session-scoped sandbox state. Cheap-clone via `Arc`; consumers
    /// (fork dispatch, SDK handler) inherit the same instance so
    /// `SandboxState::update_config` hot-reloads propagate everywhere.
    pub fn sandbox_state(&self) -> Option<Arc<coco_sandbox::SandboxState>> {
        self.sandbox_resources.sandbox_state.clone()
    }

    /// Shared multi-turn transcript for this runtime.
    pub fn history(&self) -> &Arc<Mutex<coco_messages::MessageHistory>> {
        self.history_resources.history()
    }

    /// Install the MCP handle that every per-turn engine receives via
    /// `wire_engine`. Call this after `SessionRuntime::build` returns
    /// so the bootstrap can wrap a real `McpConnectionManager`.
    pub async fn attach_mcp_handle(&self, handle: coco_tool_runtime::McpHandleRef) {
        let mut slot = self.integration_resources.mcp_handle().write().await;
        *slot = Some(handle);
    }

    /// Snapshot the installed MCP handle. `None` => no handle wired.
    pub async fn current_mcp_handle(&self) -> Option<coco_tool_runtime::McpHandleRef> {
        self.integration_resources.mcp_handle().read().await.clone()
    }

    pub(super) async fn current_mcp_manager(
        &self,
    ) -> Option<Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>> {
        self.integration_resources
            .mcp_manager()
            .read()
            .await
            .clone()
    }

    /// Install the live `McpConnectionManager` so reload paths can re-register
    /// plugin-contributed MCP servers. Call this after `SessionRuntime::build`
    /// on entry points that own a manager (the SDK path today).
    pub async fn attach_mcp_manager(
        &self,
        manager: Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>,
    ) {
        let mut slot = self.integration_resources.mcp_manager().write().await;
        *slot = Some(manager);
    }

    /// Current MCP reconnect key. Increments each time
    /// [`Self::reload_plugin_mcp_servers`] changes the registered set.
    pub fn mcp_reconnect_key(&self) -> u64 {
        self.integration_resources
            .mcp_reconnect_key()
            .load(Ordering::Relaxed)
    }

    pub(super) fn bump_mcp_reconnect_key(&self) {
        self.integration_resources
            .mcp_reconnect_key()
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Install or replace the late-bound LSP handle. Same semantics as
    /// [`Self::attach_mcp_handle`] - slot is read at every
    /// `wire_engine` call so per-turn engines pick up swaps.
    pub async fn attach_lsp_handle(&self, handle: coco_tool_runtime::LspHandleRef) {
        let mut slot = self.integration_resources.lsp_handle().write().await;
        *slot = Some(handle);
    }

    /// Snapshot the installed LSP handle. `None` => no handle wired -
    /// `wire_engine` falls back to `NoOpLspHandle` and `LspTool` hides
    /// from the model.
    pub async fn current_lsp_handle(&self) -> Option<coco_tool_runtime::LspHandleRef> {
        self.integration_resources.lsp_handle().read().await.clone()
    }

    /// Snapshot the current session id as a checked typed identity.
    pub async fn current_typed_session_id(&self) -> SessionId {
        self.engine_config_resources
            .engine_config()
            .read()
            .await
            .session_id
            .clone()
    }

    /// Synchronous mirror of the current session id.
    ///
    /// This is used only to create cheap handle snapshots. Async runtime paths
    /// should prefer [`Self::current_typed_session_id`] while the fused runtime
    /// still exists.
    pub fn current_typed_session_id_snapshot(&self) -> SessionId {
        clone_std_rwlock(self.engine_config_resources.orchestration_engine_config()).session_id
    }

    /// Whether this run persists session artifacts (transcript / usage /
    /// file-history / subagent transcripts). False under
    /// `--no-session-persistence`.
    pub fn persist_session(&self) -> bool {
        self.persistence.persist_session()
    }

    pub fn session_manager(&self) -> &Arc<coco_session::SessionManager> {
        self.persistence.session_manager()
    }

    pub fn project_paths(&self) -> &Arc<coco_paths::ProjectPaths> {
        self.persistence.project_paths()
    }

    pub fn transcript_store(&self) -> &Arc<dyn coco_session::SessionStore> {
        self.persistence.transcript_store()
    }

    pub fn fast_model_spec(&self) -> Option<&coco_types::ModelSpec> {
        self.title_resources.fast_model_spec()
    }

    pub fn auto_title_enabled(&self) -> bool {
        self.title_resources.auto_title_enabled()
    }

    pub fn original_cwd(&self) -> &std::path::PathBuf {
        self.workspace_resources.original_cwd()
    }

    pub fn project_root(&self) -> &std::path::PathBuf {
        self.workspace_resources.project_root()
    }

    pub fn current_cwd(&self) -> &Arc<RwLock<std::path::PathBuf>> {
        self.workspace_resources.current_cwd()
    }

    pub fn file_read_state(&self) -> &Arc<RwLock<coco_context::FileReadState>> {
        self.engine_state_resources.file_read_state()
    }

    pub fn file_history(&self) -> Option<&Arc<RwLock<coco_context::FileHistoryState>>> {
        self.engine_state_resources.file_history()
    }

    pub fn app_state(&self) -> &Arc<RwLock<coco_types::ToolAppState>> {
        self.engine_state_resources.app_state()
    }

    /// Apply SDK `/env` updates to the session-scoped shell environment.
    ///
    /// Empty values mean "unset", matching the historical SDK
    /// `control/updateEnv` convention. When shell tools are disabled there is
    /// no consumer, but we still count the accepted update for telemetry and
    /// protocol compatibility.
    pub fn apply_session_env_updates(
        &self,
        env: std::collections::HashMap<String, String>,
    ) -> (i32, i32) {
        let mut applied = 0_i32;
        let mut cleared = 0_i32;
        let session_env_vars = self.engine_state_resources.session_env_vars();

        for (key, value) in env {
            if value.is_empty() {
                if let Some(store) = session_env_vars {
                    store.delete(&key);
                }
                cleared += 1;
            } else {
                if let Some(store) = session_env_vars {
                    store.set(key, value);
                }
                applied += 1;
            }
        }

        (applied, cleared)
    }

    #[cfg(test)]
    pub fn session_env_snapshot(&self) -> Option<std::collections::HashMap<String, String>> {
        self.engine_state_resources
            .session_env_vars()
            .map(coco_shell::SessionEnvVars::snapshot)
    }

    pub fn loop_sentinel_state(
        &self,
    ) -> &Arc<Mutex<coco_skills::bundled::loop_skill::LoopSentinelState>> {
        self.engine_state_resources.loop_sentinel_state()
    }

    pub fn config_home(&self) -> &std::path::PathBuf {
        self.config_resources.config_home()
    }

    pub fn runtime_config(&self) -> &Arc<coco_config::RuntimeConfig> {
        self.config_resources.runtime_config()
    }

    pub fn process_runtime(&self) -> &Arc<crate::process_runtime::ProcessRuntime> {
        self.project_resources.process_runtime()
    }

    pub fn project_services(&self) -> &Arc<crate::project_services::ProjectServices> {
        self.project_resources.project_services()
    }

    pub async fn flush_session_usage_snapshot(&self) {
        self.turn_resources
            .usage_accounting()
            .flush_snapshot()
            .await;
    }

    pub async fn session_usage_snapshot(&self) -> coco_types::SessionUsageSnapshot {
        self.turn_resources.usage_accounting().snapshot().await
    }

    pub async fn persist_goal_metadata(&self, goal: Option<coco_session::GoalMetadata>) {
        if !self.persistence.persist_session() {
            return;
        }
        self.engine_state_resources
            .terminal_goal_metadata_written()
            .store(goal.as_ref().is_some_and(|goal| goal.met), Ordering::SeqCst);
        let session_id = self.current_typed_session_id().await;
        let session_id_string = session_id.to_string();
        let store = Arc::clone(self.persistence.transcript_store());
        let entry = coco_session::MetadataEntry::Goal {
            session_id: session_id.clone(),
            goal,
        };
        let session_id_for_write = session_id_string;
        match tokio::task::spawn_blocking(move || {
            store.append_metadata(&session_id_for_write, &entry)
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                warn!(error = %e, session_id = %session_id, "failed to persist goal metadata");
            }
            Err(e) => {
                warn!(error = %e, session_id = %session_id, "goal metadata write task failed");
            }
        }
    }

    pub async fn persist_local_transcript_messages(&self, messages: &[coco_messages::Message]) {
        if !self.persistence.persist_session() || messages.is_empty() {
            return;
        }
        let session_id = self.current_typed_session_id().await;
        let session_id_string = session_id.to_string();
        let store = Arc::clone(self.persistence.transcript_store());
        let seen = Arc::clone(self.engine_state_resources.transcript_dedup());
        let cwd_path = self.current_cwd().read().await.clone();
        let cwd = cwd_path.display().to_string();
        let git_branch = coco_git::get_current_branch(&cwd_path)
            .ok()
            .flatten()
            .filter(|s| !s.is_empty());
        let now = chrono::Utc::now().to_rfc3339();
        let message_owned = messages.to_vec();
        let mut seen_guard = seen.lock().await;
        let message_refs: Vec<&coco_messages::Message> = message_owned.iter().collect();
        let options = coco_session::storage::ChainWriteOptions {
            cwd,
            timestamp: now,
            is_sidechain: false,
            agent_id: None,
            starting_parent_uuid: None,
            git_branch,
        };
        if let Err(e) =
            store.append_message_chain(&session_id_string, &message_refs, &mut seen_guard, options)
        {
            warn!(error = %e, session_id = %session_id, "failed to persist local transcript messages");
        }
    }

    pub async fn pre_clear_rewind_messages(&self) -> Option<Vec<Arc<Message>>> {
        self.engine_state_resources
            .clear_rewind_messages()
            .lock()
            .await
            .clone()
    }

    pub async fn restore_pre_clear_rewind_prefix(
        &self,
        message_id: &str,
    ) -> Option<(i32, i32, Vec<Message>)> {
        let messages = self
            .engine_state_resources
            .clear_rewind_messages()
            .lock()
            .await
            .clone()?;
        let idx = messages.iter().position(|m| match m.as_ref() {
            Message::User(u) => u.uuid.to_string() == message_id,
            _ => false,
        })?;
        let selected_prompt =
            coco_messages::wrapping::extract_text_from_message(messages[idx].as_ref());
        let pre_count = messages.len() as i32;
        let kept: Vec<Message> = messages[..idx]
            .iter()
            .map(|message| message.as_ref().clone())
            .collect();
        let messages_removed = (pre_count - idx as i32).max(0);
        {
            let mut history = self.history_resources.history().lock().await;
            let replacement = kept.iter().cloned().map(Arc::new).collect();
            let no_event_tx = None;
            coco_query::history_sync::history_replace_and_emit(
                &mut history,
                replacement,
                &no_event_tx,
                coco_types::HistoryReplaceReason::Rewind,
            )
            .await;
        }
        self.persist_local_transcript_messages(&kept).await;
        let session_id = self.current_typed_session_id().await;
        let session_id_string = session_id.to_string();
        if let Err(e) = self.persistence.transcript_store().append_metadata(
            &session_id_string,
            &coco_session::MetadataEntry::LastPrompt {
                session_id: session_id.clone(),
                last_prompt: selected_prompt.trim().to_string(),
                leaf_uuid: Some(message_id.to_string()),
                explicit: true,
                rewound: true,
            },
        ) {
            warn!(error = %e, session_id = %session_id, message_id, "failed to persist rewind last-prompt metadata");
        }
        Some((idx as i32, messages_removed, kept))
    }

    pub fn side_query(&self) -> coco_tool_runtime::SideQueryHandle {
        self.turn_resources.side_query()
    }

    pub(crate) fn usage_accounting(&self) -> coco_query::usage_accounting::UsageAccounting {
        self.turn_resources.usage_accounting()
    }

    pub async fn install_side_query_event_tx(&self, event_tx: mpsc::Sender<coco_query::CoreEvent>) {
        self.turn_resources
            .usage_accounting()
            .install_event_tx(event_tx)
            .await;
    }

    /// Generate the on-demand LLM risk explanation for a permission prompt.
    /// Runs the explainer via the session `SideQuery` handle, gated on
    /// `permission_explainer_enabled` (default-on) and bounded by a timeout.
    /// Graceful-degrades to `None` when the setting is off, the side query
    /// errors, or the timeout elapses. The single home for the explainer call
    /// - `TuiPermissionBridge::explain_risk` and the tui_runner Ctrl+E path
    /// both delegate here.
    pub async fn explain_permission_risk(
        &self,
        params: coco_permissions::ExplainerParams<'_>,
    ) -> Option<coco_types::PermissionExplanation> {
        if !self
            .runtime_config()
            .settings
            .merged
            .permissions
            .explainer_enabled()
        {
            return None;
        }
        let handle = self.side_query();
        let fut =
            coco_permissions::generate_permission_explanation(params, move |req| async move {
                handle.query(req).await.map_err(|e| e.to_string())
            });
        // Bound the timeout so a slow/hung side query can't pin the explainer panel.
        tokio::time::timeout(std::time::Duration::from_secs(8), fut)
            .await
            .unwrap_or_default()
    }

    pub fn model_runtimes(&self) -> Arc<coco_inference::ModelRuntimeRegistry> {
        self.execution.model_runtimes()
    }

    /// Resolve an SDK/user-supplied model string into a concrete
    /// provider/model pair using the same registry snapshot that built
    /// this session. `provider/model_id` is accepted directly; bare
    /// model ids first bind to the current Main provider, then to the
    /// deterministic provider catalog order.
    pub fn resolve_model_selection(&self, raw_model: &str) -> Option<ProviderModelSelection> {
        resolve_model_selection_from_runtime_config(self.runtime_config(), raw_model)
    }

    /// Best-effort PID-registry live patch - push the human-readable
    /// session name into the `<config_home>/sessions/<pid>.json` file
    /// that `coco ps` reads. Silently no-ops when the session isn't
    /// registered (subagent context, FS-constrained startup, etc.)
    /// because the PID registry guard is optional. Without this patch the live
    /// registry shows the stale startup name forever.
    pub fn update_session_registry_name(&self, name: &str) {
        self.lifecycle_resources.update_session_registry_name(name);
    }

    /// Seed the transcript dedup set with uuids that are already
    /// persisted on disk. Called on resume / fork so the first
    /// post-load turn doesn't re-write the loaded messages.
    /// MUST clear the dedup set first. In-TUI `/resume` reuses the
    /// runtime, so without the clear the prior session's UUIDs leak
    /// into the new session and any colliding new write gets silently
    /// suppressed.
    pub async fn seed_transcript_dedup<I>(&self, uuids: I)
    where
        I: IntoIterator<Item = uuid::Uuid>,
    {
        let mut g = self.engine_state_resources.transcript_dedup().lock().await;
        g.clear();
        g.extend(uuids);
    }

    /// Reconstruct Level 2 tool-result replacement state from the
    /// restored messages plus transcript content-replacement records.
    /// Called on resume/fork before the first resumed turn.
    /// `agent_id` MUST be the runtime's current agent_id (None for
    /// main-thread sessions, Some for subagents). The transcript
    /// content-replacement records are stamped with `agent_id` at
    /// write time (`engine_prompt.rs:200-216`); reading with
    /// `agent_id: None` for a subagent resume would silently drop
    /// every Level-2 replacement and force the model to re-read the
    /// full tool result, breaking prompt-cache stability.
    pub async fn seed_tool_result_replacement_state(
        &self,
        messages: &[Message],
        session_id: &SessionId,
        agent_id: Option<&str>,
    ) {
        let records = self
            .persistence
            .transcript_store()
            .load_content_replacements_for_chain(session_id.as_str(), agent_id)
            .unwrap_or_default();
        let mut next =
            coco_tool_runtime::tool_result_storage::ContentReplacementState::new(i64::MAX);
        for msg in messages {
            if let Message::ToolResult(tr) = msg {
                next.seen_ids.insert(tr.tool_use_id.clone());
            }
        }
        for record in records {
            next.seen_ids.insert(record.tool_use_id().to_string());
            next.replacements.insert(
                record.tool_use_id().to_string(),
                record.replacement().to_string(),
            );
        }
        *self
            .engine_state_resources
            .tool_result_replacement_state()
            .write()
            .await = next;
    }

    /// Borrow the optional `MemoryRuntime`. `None` when
    /// `Feature::AutoMemory` is off. Callers (e.g. the slash dispatcher's
    /// `/dream` and `/summary` triggers) clone the inner `Arc`.
    pub fn memory_runtime(&self) -> Option<&Arc<coco_memory::MemoryRuntime>> {
        self.memory_resources.memory_runtime.as_ref()
    }

    /// The production swarm `AgentHandle` once `attach_agent_handle` has
    /// late-bound it (the eager `swarm_agent_handle` is a no-op until then).
    /// `None` before attach / in non-swarm sessions. Used by the leader
    /// inbox poller to resolve the active team via `active_team_name`.
    pub async fn current_agent_handle(&self) -> Option<AgentHandleRef> {
        self.handle_resources.agent_handle.read().await.clone()
    }

    /// Public accessor for the hook registry. Same `Arc` as the one
    /// installed on every per-turn engine; safe to clone.
    pub fn hook_registry(&self) -> Arc<coco_hooks::HookRegistry> {
        self.hook_resources.registry()
    }

    /// Public accessor for the session-scoped [`coco_skills::SkillManager`].
    /// Same `Arc` that backed the command-registry build and the
    /// reminder pipeline - safe to clone (cheap ref-count bump).
    /// Used by binary-entry wiring (e.g. `mcp_handle_adapter`) that
    /// sits outside the crate's `pub(crate)` field-access scope.
    pub fn skill_manager(&self) -> Arc<coco_skills::SkillManager> {
        Arc::clone(self.catalog_resources.skill_manager())
    }

    pub fn command_registry_slot(
        &self,
    ) -> &Arc<tokio::sync::RwLock<Arc<coco_commands::CommandRegistry>>> {
        self.catalog_resources.command_registry()
    }

    /// Session-scoped command queue handle. Producers outside the
    /// per-turn engine - the TUI bridge in `tui_runner` (user typing
    /// while busy), future task-completion / coordinator / hook
    /// forwarders - call `enqueue` on this handle to inject mid-turn
    /// steering messages. Returned by reference; callers `.clone()` if
    /// they need an owned `Arc`-backed handle.
    /// Teammate messages and task notifications use the same queue
    /// with `QueueOrigin::Coordinator` / `QueueOrigin::TaskNotification`.
    pub fn command_queue(&self) -> &CommandQueue {
        self.command_resources.command_queue()
    }

    /// The session's schedule store (cron tasks + triggers). Shared with the
    /// cron tick driver ([`crate::cron_tick`]) so it reads/writes the same
    /// tasks the `Cron*` tools persist.
    pub fn schedule_store(&self) -> coco_tool_runtime::ScheduleStoreRef {
        self.turn_resources.schedule_store()
    }

    /// Session shutdown signal - long-lived background tasks (e.g. the cron
    /// tick) observe it for clean teardown.
    pub fn shutdown_signal(&self) -> CancellationToken {
        self.lifecycle_resources.cancel()
    }

    /// Build a closure that materialises an
    /// [`coco_hooks::orchestration::OrchestrationContext`] tied to the
    /// current session's identity / cwd / disable flags.
    /// Used by detached hook firings (e.g. the `Elicitation` /
    /// `ElicitationResult` wrapper around `SendElicitation`, the
    /// FileChanged file watcher) that need a context built from inside
    /// a sync closure. Each call reads the synchronous snapshot mirrors
    /// kept up to date by session/config mutations, avoiding Tokio
    /// `blocking_read()` on runtime worker threads.
    pub fn orchestration_ctx_factory(
        self: &Arc<Self>,
    ) -> Arc<dyn Fn() -> coco_hooks::orchestration::OrchestrationContext + Send + Sync> {
        let runtime = self.clone();
        Arc::new(move || {
            let cfg = clone_std_rwlock(
                runtime
                    .engine_config_resources
                    .orchestration_engine_config(),
            );
            coco_hooks::orchestration::OrchestrationContext {
                session_id: cfg.session_id.clone(),
                cwd: cfg.workspace_cwd(),
                project_dir: cfg.project_dir.clone(),
                permission_mode: None,
                transcript_path: None,
                agent_id: None,
                agent_type: None,
                cancel: runtime.lifecycle_resources.cancel(),
                disable_all_hooks: cfg.disable_all_hooks,
                allow_managed_hooks_only: cfg.allow_managed_hooks_only,
                attachment_emitter: coco_messages::AttachmentEmitter::noop(),
                sync_event_sink: None,
                http_url_allowlist: None,
                http_env_var_policy: None,
                async_registry: Some(runtime.hook_resources.async_registry()),
                async_rewake_sink: Some(async_rewake_sink(
                    runtime.command_resources.command_queue(),
                )),
                llm_handle: Some(runtime.hook_resources.llm_handle()),
                workspace_trust_accepted: None,
            }
        })
    }

    /// Snapshot the current `QueryEngineConfig` (clones the inner struct).
    /// Per-turn engine builds use this so mid-session mutations like
    /// `set_permission_mode` propagate immediately.
    pub async fn current_engine_config(&self) -> QueryEngineConfig {
        self.engine_config_resources
            .engine_config()
            .read()
            .await
            .clone()
    }

    /// Mutate `engine_config` under lock. Use for mid-session updates
    /// like `SetPermissionMode`.
    pub async fn update_engine_config<F>(&self, f: F)
    where
        F: FnOnce(&mut QueryEngineConfig),
    {
        let snapshot = {
            let mut g = self.engine_config_resources.engine_config().write().await;
            f(&mut g);
            g.clone()
        };
        write_std_rwlock(
            self.engine_config_resources.orchestration_engine_config(),
            snapshot,
        );
    }

    pub async fn seed_todo_list_snapshot(&self, key: String, items: Vec<coco_types::TodoRecord>) {
        let handle = self.handle_resources.todo_list.read().await.clone();
        handle.write(&key, items.clone()).await;
        let mut app_state = self.engine_state_resources.app_state().write().await;
        if items.is_empty() {
            app_state.todos_by_agent.remove(&key);
        } else {
            app_state.todos_by_agent.insert(key, items);
        }
    }

    pub async fn todo_list_snapshot(&self, key: &str) -> Vec<coco_types::TodoRecord> {
        self.handle_resources.todo_list.read().await.read(key).await
    }
}
