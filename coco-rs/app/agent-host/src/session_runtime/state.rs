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
    /// the client-hosted MCP lifecycle handlers) use this to mutate the registry
    /// via its interior-mutability API.
    pub fn tools(&self) -> &Arc<ToolRegistry> {
        self.execution.tools()
    }

    /// Session-scoped sandbox state. Cheap-clone via `Arc`; consumers
    /// (fork dispatch, AppServer adapters) inherit the same instance so
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
    /// on entry points that own a manager (the AppServer path today).
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

    /// Apply AppServer `control/updateEnv` updates to the session-scoped shell environment.
    ///
    /// Empty values mean "unset", matching the remote
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

    pub async fn prompt_history_texts(&self, project: String) -> Vec<String> {
        let config_home = self.config_home().clone();
        let session_id = self.current_typed_session_id().await;
        tokio::task::spawn_blocking(move || {
            coco_session::PromptHistory::new(&config_home, &project, &session_id)
                .get_history()
                .into_iter()
                .map(|entry| entry.display)
                .collect()
        })
        .await
        .unwrap_or_default()
    }

    pub async fn persist_prompt_history_entry(
        &self,
        project: String,
        display: String,
    ) -> anyhow::Result<()> {
        let config_home = self.config_home().clone();
        let session_id = self.current_typed_session_id().await;
        tokio::task::spawn_blocking(move || {
            coco_session::PromptHistory::new(&config_home, &project, &session_id)
                .add(&display)
                .map_err(anyhow::Error::from)
        })
        .await?
    }

    pub async fn clear_awaiting_plan_approval_if_matches(&self, request_id: &str) -> bool {
        let mut guard = self.engine_state_resources.app_state().write().await;
        if guard.awaiting_plan_approval_request_id.as_deref() != Some(request_id) {
            return false;
        }
        guard.awaiting_plan_approval = false;
        guard.awaiting_plan_approval_request_id = None;
        true
    }

    pub async fn has_exited_plan_mode(&self) -> bool {
        self.engine_state_resources
            .app_state()
            .read()
            .await
            .has_exited_plan_mode
    }

    pub async fn set_agent_progress_summaries_enabled(&self, enabled: bool) {
        self.engine_state_resources
            .app_state()
            .write()
            .await
            .agent_progress_summaries_enabled = enabled;
    }

    pub fn configured_plans_dir(&self) -> std::path::PathBuf {
        coco_context::resolve_plans_directory(
            self.config_home(),
            self.runtime_config().paths.project_dir.as_deref(),
            self.runtime_config()
                .settings
                .merged
                .plans_directory
                .as_deref(),
        )
    }

    pub fn session_plan_file_path(&self) -> std::path::PathBuf {
        let plans_dir = self.configured_plans_dir();
        let session_id = self.current_typed_session_id_snapshot();
        coco_context::get_plan_file_path(session_id.as_str(), &plans_dir, /*agent_id*/ None)
    }

    pub fn unscoped_session_plan_text(&self, session_id: &coco_types::SessionId) -> Option<String> {
        let plans_dir = coco_context::resolve_plans_directory(
            self.config_home(),
            /*project_dir*/ None,
            /*setting*/ None,
        );
        coco_context::get_plan(session_id.as_str(), &plans_dir, /*agent_id*/ None)
    }

    pub fn runtime_config(&self) -> &Arc<coco_config::RuntimeConfig> {
        self.config_resources.runtime_config()
    }

    pub fn process_runtime(&self) -> &Arc<coco_app_runtime::ProcessRuntime> {
        self.project_resources.process_runtime()
    }

    pub fn project_services(&self) -> &Arc<coco_app_runtime::ProjectServices> {
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

    pub async fn active_goal_snapshot(&self) -> Option<coco_types::ActiveGoal> {
        self.engine_state_resources
            .app_state()
            .read()
            .await
            .active_goal
            .clone()
    }

    pub async fn restore_goal_from_history(
        &self,
        messages: &[Arc<coco_messages::Message>],
        trust_rejected: bool,
    ) -> Option<coco_types::ActiveGoal> {
        let cfg = self.current_engine_config().await;
        let goal = crate::goal_command::restore_goal_from_history(
            messages,
            self.engine_state_resources.app_state(),
            self.hook_resources.registry().as_ref(),
            self.session_usage_snapshot().await.totals.output_tokens,
            crate::goal_command::GoalGate {
                hooks_restricted: cfg.disable_all_hooks || cfg.allow_managed_hooks_only,
                trust_rejected,
            },
        )
        .await;
        self.persist_goal_metadata(goal.as_ref().map(|goal| {
            coco_session::GoalMetadata::from_active_goal(goal, /*met*/ false)
        }))
        .await;
        goal
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

    pub async fn append_messages_to_history(
        &self,
        messages: Vec<coco_messages::Message>,
    ) -> Vec<Arc<coco_messages::Message>> {
        let mut history = self.history_resources.history().lock().await;
        messages
            .into_iter()
            .map(|message| {
                let message = Arc::new(message);
                history.push_arc(message.clone());
                message
            })
            .collect()
    }

    pub async fn append_messages_to_history_and_emit(
        &self,
        messages: Vec<coco_messages::Message>,
        event_tx: Option<mpsc::Sender<coco_types::CoreEvent>>,
    ) -> Vec<Arc<coco_messages::Message>> {
        let mut history = self.history_resources.history().lock().await;
        for message in messages {
            coco_query::history_sync::history_push_and_emit(&mut history, message, &event_tx).await;
        }
        history.to_vec()
    }

    pub async fn history_messages(&self) -> Vec<Arc<coco_messages::Message>> {
        self.history_resources.history().lock().await.to_vec()
    }

    pub async fn truncate_history_at_user_message(
        &self,
        message_id: &str,
    ) -> Result<super::SessionHistoryTruncateResult, usize> {
        let mut history = self.history_resources.history().lock().await;
        let Some(idx) = history
            .as_slice()
            .iter()
            .position(|message| match message.as_ref() {
                Message::User(user) => user.uuid.to_string() == message_id,
                _ => false,
            })
        else {
            return Err(history.len());
        };
        let pre_count = history.len();
        history.truncate(idx);
        Ok(super::SessionHistoryTruncateResult {
            keep_count: idx,
            pre_count,
            removed: pre_count.saturating_sub(idx),
        })
    }

    pub async fn append_arc_messages_to_history_and_snapshot(
        &self,
        messages: Vec<Arc<coco_messages::Message>>,
    ) -> Vec<Arc<coco_messages::Message>> {
        let mut history = self.history_resources.history().lock().await;
        for message in messages {
            history.push_arc(message);
        }
        history.to_vec()
    }

    async fn replace_history(&self, history: coco_messages::MessageHistory) {
        *self.history_resources.history().lock().await = history;
    }

    pub async fn replace_history_with_arc_messages(
        &self,
        messages: Vec<Arc<coco_messages::Message>>,
    ) {
        let mut history = coco_messages::MessageHistory::new();
        for message in messages {
            history.push_arc(message);
        }
        self.replace_history(history).await;
    }

    pub async fn commit_engine_turn_history(&self, history: coco_messages::MessageHistory) {
        self.replace_history(history).await;
    }

    pub async fn commit_compacted_history(&self, history: coco_messages::MessageHistory) {
        self.replace_history(history).await;
    }

    pub async fn re_append_session_metadata(&self) {
        let session_id = self.current_typed_session_id().await.to_string();
        let manager = Arc::clone(self.session_manager());
        let _ =
            tokio::task::spawn_blocking(move || manager.re_append_session_metadata(&session_id))
                .await;
    }

    pub async fn has_persisted_title(&self) -> bool {
        let session_id = self.current_typed_session_id().await.to_string();
        let manager = Arc::clone(self.session_manager());
        tokio::task::spawn_blocking(move || {
            manager
                .load(&session_id)
                .map(|session| session.title.is_some())
                .unwrap_or(false)
        })
        .await
        .unwrap_or(false)
    }

    pub async fn persist_session_title(&self, name: String) -> anyhow::Result<()> {
        let session_id = self.current_typed_session_id().await.to_string();
        let manager = Arc::clone(self.session_manager());
        tokio::task::spawn_blocking(move || manager.set_title(&session_id, &name))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|inner| inner.map_err(anyhow::Error::from))
            .map(|_| ())
    }

    pub async fn title_generation_conversation_text(&self) -> String {
        let history = self.history_resources.history().lock().await;
        coco_session::title_generator::extract_conversation_text(history.as_slice())
    }

    pub async fn list_persisted_session_summaries(
        &self,
    ) -> anyhow::Result<coco_types::SessionListResult> {
        let manager = Arc::clone(self.session_manager());
        tokio::task::spawn_blocking(move || {
            let sessions = manager.list()?;
            let summaries = sessions
                .into_iter()
                .map(|session| {
                    Ok(coco_types::SessionSummary {
                        session_id: coco_types::SessionId::try_new(session.id)?,
                        model: session.model,
                        cwd: session.working_dir.to_string_lossy().into_owned(),
                        created_at: session.created_at,
                        updated_at: session.updated_at,
                        title: session.title,
                        message_count: session.message_count,
                        total_tokens: session.total_tokens,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(coco_types::SessionListResult {
                sessions: summaries,
            })
        })
        .await?
    }

    pub async fn persist_session_mode(&self) {
        let session_id = self.current_typed_session_id().await;
        let manager = Arc::clone(self.session_manager());
        let features = self.runtime_config().features.clone();
        let _ = tokio::task::spawn_blocking(move || {
            crate::coordinator_mode_resume::persist_session_mode(
                manager.as_ref(),
                &session_id,
                &features,
            )
        })
        .await;
    }

    pub fn reconcile_session_mode_on_resume(
        &self,
        stored_mode: Option<&str>,
    ) -> Option<&'static str> {
        crate::coordinator_mode_resume::reconcile_on_resume(
            stored_mode,
            &self.runtime_config().features,
        )
    }

    pub async fn toggle_tag(&self, tag: String) -> anyhow::Result<(SessionId, bool)> {
        let session_id = self.current_typed_session_id().await;
        let session_id_for_toggle = session_id.to_string();
        let manager = Arc::clone(self.session_manager());
        let (_, added) =
            tokio::task::spawn_blocking(move || manager.toggle_tag(&session_id_for_toggle, &tag))
                .await
                .map_err(anyhow::Error::from)?
                .map_err(anyhow::Error::from)?;
        Ok((session_id, added))
    }

    pub async fn rewind_files(
        &self,
        request: super::SessionFileRewindRequest,
    ) -> Result<super::SessionFileRewindResult, super::SessionFileRewindError> {
        let Some(history_arc) = self.file_history().cloned() else {
            return Err(super::SessionFileRewindError::NotEnabled);
        };
        let session_id = self.current_typed_session_id().await;
        let config_home = self.config_home().clone();

        {
            let history = history_arc.read().await;
            if !history.can_restore(&request.user_message_id) {
                return Err(super::SessionFileRewindError::SnapshotMissing(
                    request.user_message_id,
                ));
            }
        }

        let stats = {
            let history = history_arc.read().await;
            history
                .get_diff_stats(&request.user_message_id, &config_home, session_id.as_str())
                .await
                .map_err(|source| super::SessionFileRewindError::Operation {
                    context: if request.dry_run {
                        "file rewind dry run"
                    } else {
                        "file rewind preview"
                    },
                    source: anyhow::Error::from(source),
                })?
        };

        if request.dry_run {
            return Ok(super::SessionFileRewindResult {
                files_changed: stats.files_changed,
                insertions: stats.insertions,
                deletions: stats.deletions,
                dry_run: true,
            });
        }

        let restored = {
            let history = history_arc.read().await;
            history
                .rewind(&request.user_message_id, &config_home, session_id.as_str())
                .await
                .map_err(|source| super::SessionFileRewindError::Operation {
                    context: "file rewind apply",
                    source: anyhow::Error::from(source),
                })?
        };

        Ok(super::SessionFileRewindResult {
            files_changed: restored,
            insertions: stats.insertions,
            deletions: stats.deletions,
            dry_run: false,
        })
    }

    pub async fn render_session_file_diff(
        &self,
    ) -> Result<coco_context::RenderedDiff, super::SessionFileDiffError> {
        let Some(history_arc) = self.file_history().cloned() else {
            return Err(super::SessionFileDiffError::NotEnabled);
        };
        let session_id = self.current_typed_session_id().await.to_string();
        let config_home = self.config_home().clone();
        let file_history = history_arc.read().await;
        file_history
            .render_session_diff(&config_home, &session_id)
            .await
            .map_err(|source| super::SessionFileDiffError::Operation {
                context: "session file diff",
                source: anyhow::Error::from(source),
            })
    }

    pub async fn rewind_diff_stats(
        &self,
        message_id: &str,
    ) -> Result<Option<coco_context::DiffStats>, super::SessionFileDiffError> {
        self.rewind_diff_stats_between(message_id, None).await
    }

    pub async fn rewind_diff_stats_between(
        &self,
        message_id: &str,
        next_message_id: Option<&str>,
    ) -> Result<Option<coco_context::DiffStats>, super::SessionFileDiffError> {
        let Some(history_arc) = self.file_history().cloned() else {
            return Err(super::SessionFileDiffError::NotEnabled);
        };
        let session_id = self.current_typed_session_id().await.to_string();
        let config_home = self.config_home().clone();
        let file_history = history_arc.read().await;
        if !file_history.can_restore(message_id) {
            return Ok(None);
        }
        file_history
            .get_diff_stats_between(message_id, next_message_id, &config_home, &session_id)
            .await
            .map(Some)
            .map_err(|source| super::SessionFileDiffError::Operation {
                context: "rewind diff stats",
                source: anyhow::Error::from(source),
            })
    }

    pub async fn render_turn_file_diff(
        &self,
        message_id: &str,
    ) -> Result<coco_context::RenderedDiff, super::SessionFileDiffError> {
        let Some(history_arc) = self.file_history().cloned() else {
            return Err(super::SessionFileDiffError::NotEnabled);
        };
        let session_id = self.current_typed_session_id().await.to_string();
        let config_home = self.config_home().clone();
        let file_history = history_arc.read().await;
        let Some(next_message_id) = next_file_history_snapshot_id(&file_history, message_id) else {
            return Err(super::SessionFileDiffError::SnapshotMissing(
                message_id.to_string(),
            ));
        };
        file_history
            .render_diff_between(
                message_id,
                next_message_id.as_deref(),
                &config_home,
                &session_id,
            )
            .await
            .map_err(|source| super::SessionFileDiffError::Operation {
                context: "turn file diff",
                source: anyhow::Error::from(source),
            })
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

    /// Resolve a client/user-supplied model string into a concrete
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
    pub(crate) async fn seed_transcript_dedup<I>(&self, uuids: I)
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
    pub(crate) async fn seed_tool_result_replacement_state(
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
            coco_tool_runtime::tool_result_offload::ContentReplacementState::new(i64::MAX);
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
    /// sits outside the crate's `pub (crate)` field-access scope.
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

    /// Session shutdown signal — the root session cancellation token. Kept
    /// `pub (crate)`: the root token must not be cancellable from
    /// an arbitrary `SessionHandle` clone outside this crate, only through the
    /// registry close path. Observers that merely watch for teardown should
    /// take [`Self::shutdown_child_token`] instead so they cannot cancel the
    /// root.
    pub(crate) fn shutdown_signal(&self) -> CancellationToken {
        self.lifecycle_resources.cancel()
    }

    /// A child of the root shutdown token for observe-only background tasks
    /// (e.g. the memory-pressure shell reaper): it is cancelled when the root
    /// is, but cancelling it cannot tear down the session.
    pub(crate) fn shutdown_child_token(&self) -> CancellationToken {
        self.shutdown_signal().child_token()
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
            // session identity is frozen through this seam. A mutation
            // that rotated `session_id` would split-brain the immutable
            // `SessionHandle` snapshot, the seq allocator's per-session domain,
            // and the persisted transcript. Enforce until identity is moved out
            // of the mutable engine-config surface entirely.
            let session_id_before = g.session_id.clone();
            f(&mut g);
            assert_eq!(
                g.session_id, session_id_before,
                "update_engine_config must not rotate the session id (H-12)"
            );
            g.clone()
        };
        write_std_rwlock(
            self.engine_config_resources.orchestration_engine_config(),
            snapshot,
        );
    }

    pub async fn set_model_id(&self, model_id: String) -> String {
        let old_model = self.current_engine_config().await.model_id;
        self.update_engine_config(move |engine_config| {
            engine_config.model_id = model_id;
        })
        .await;
        old_model
    }

    pub async fn set_thinking_level(&self, thinking_level: Option<coco_types::ThinkingLevel>) {
        self.update_engine_config(move |engine_config| {
            engine_config.thinking_level = thinking_level;
        })
        .await;
    }

    pub async fn set_fast_mode(&self, active: bool) {
        self.update_engine_config(move |engine_config| {
            engine_config.fast_mode = active;
        })
        .await;
    }

    pub async fn set_requires_structured_output(&self, active: bool) {
        self.update_engine_config(move |engine_config| {
            engine_config.requires_structured_output = active;
        })
        .await;
    }

    pub async fn set_skill_overrides(&self, skill_overrides: Arc<coco_config::SkillOverrideTiers>) {
        self.update_engine_config(move |engine_config| {
            engine_config.skill_overrides = skill_overrides;
        })
        .await;
    }

    pub async fn apply_session_start_config(&self, config: super::SessionStartRuntimeConfig) {
        let model_id = config.model_id;
        let permission_mode = config.permission_mode;
        let plan_mode_custom_instructions = config.plan_mode_custom_instructions;
        let requires_structured_output = config.requires_structured_output;
        self.update_engine_config(move |engine_config| {
            if let Some(model_id) = model_id {
                engine_config.model_id = model_id;
            }
            if let Some(permission_mode) = permission_mode {
                engine_config.permission_mode = permission_mode;
            }
            if let Some(custom_instructions) = plan_mode_custom_instructions {
                engine_config.plan_mode_settings.custom_instructions = custom_instructions;
            }
            if requires_structured_output {
                engine_config.requires_structured_output = true;
            }
        })
        .await;

        if permission_mode.is_none() && !config.agent_progress_summaries_enabled {
            return;
        }

        let mut app_state = self.engine_state_resources.app_state().write().await;
        if let Some(mode) = permission_mode {
            // Brand-new session: the engine config / rules are not part of a
            // turn build yet, so the Auto-entry stash starts empty. The
            // evaluator-facing strip in ToolContextFactory::build, keyed on
            // live mode==Auto, is the runtime guard once a turn starts.
            let live_allow_rules = coco_types::PermissionRulesBySource::new();
            let previous = app_state
                .permissions
                .mode
                .unwrap_or(coco_types::PermissionMode::Default);
            coco_permissions::apply_permission_mode_transition_to_app_state(
                &mut app_state,
                previous,
                mode,
                &live_allow_rules,
                coco_permissions::PlanModeAutoOptions::default(),
            );
        }
        if config.agent_progress_summaries_enabled {
            app_state.agent_progress_summaries_enabled = true;
        }
    }

    pub async fn apply_turn_runtime_config(&self, config: super::SessionTurnRuntimeConfig) {
        self.update_engine_config(move |engine_config| {
            engine_config.is_non_interactive = config.is_non_interactive;
            engine_config.avoid_permission_prompts = config.avoid_permission_prompts;
            engine_config.permission_mode = config.permission_mode;
            engine_config.permission_mode_availability = config.permission_mode_availability;
            engine_config.permission_rule_source_roots = config.permission_rule_source_roots;
            engine_config.max_turns = config.max_turns;
            engine_config.total_token_budget = config.total_token_budget;
            engine_config.cwd_override = config.cwd_override;
            engine_config.tool_filter = config.tool_filter;
            engine_config.plans_directory = config.plans_directory;
            engine_config.plan_mode_settings.custom_instructions =
                config.plan_mode_custom_instructions;
        })
        .await;
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

    pub async fn set_agent_color(&self, color: Option<coco_types::AgentColorName>) {
        self.engine_state_resources
            .app_state()
            .write()
            .await
            .agent_color = color;
    }

    pub async fn todo_list_snapshot(&self, key: &str) -> Vec<coco_types::TodoRecord> {
        self.handle_resources.todo_list.read().await.read(key).await
    }
}

fn next_file_history_snapshot_id(
    file_history: &coco_context::FileHistoryState,
    message_id: &str,
) -> Option<Option<String>> {
    let idx = file_history
        .snapshots
        .iter()
        .position(|snapshot| snapshot.message_id == message_id)?;
    Some(
        file_history
            .snapshots
            .get(idx + 1)
            .map(|snapshot| snapshot.message_id.clone()),
    )
}
