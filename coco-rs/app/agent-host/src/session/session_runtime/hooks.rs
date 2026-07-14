use std::path::PathBuf;
use std::sync::Arc;

use tracing::info;
use tracing::warn;

use super::SessionHandle;
use super::SessionRuntime;

use coco_query::CommandQueue;
use coco_query::QueryEngineConfig;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use coco_app_runtime::ProjectServices;

#[derive(Clone)]
pub(super) struct FileWatchRegistrationContext {
    pub(super) file_changed_watcher:
        Arc<RwLock<Option<crate::file_changed_watcher::FileChangedHookWatcher>>>,
    pub(super) hook_registry: Arc<coco_hooks::HookRegistry>,
    pub(super) session_id: coco_types::SessionId,
    pub(super) engine_config: Arc<RwLock<QueryEngineConfig>>,
    pub(super) cancel: CancellationToken,
    pub(super) async_hook_registry: Arc<coco_hooks::async_registry::AsyncHookRegistry>,
    pub(super) command_queue: CommandQueue,
    pub(super) hook_llm_handle: Arc<dyn coco_hooks::HookLlmHandle>,
}

pub(super) fn async_rewake_sink(queue: &CommandQueue) -> Arc<dyn coco_hooks::AsyncRewakeSink> {
    Arc::new(crate::command_queue_sink::CommandQueueNotificationSink::new(queue.clone()))
}

/// Spawn a TUI-lifetime ConfigChange watcher that resolves the active
/// [`SessionHandle`] for every reload event.
///
/// This mirrors the skill/cron watchers: hook execution targets the current
/// runtime, while the TUI owner reattaches this subscription whenever
/// `/resume`, `/branch`, or `/clear` installs a replacement runtime.
pub fn spawn_current_session_config_change_watcher(
    current_session: Arc<RwLock<SessionHandle>>,
    mut rx: tokio::sync::broadcast::Receiver<coco_config_reload::ConfigChange>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(change) => {
                    let source = config_change_source_for_kind(change.kind);
                    let path = change.path.to_string_lossy().into_owned();
                    let session = current_session.read().await.clone();
                    session.fire_config_change_hooks(source, Some(&path)).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "ConfigChange watcher lagged; events dropped");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

/// Populate a `HookRegistry` from the current `RuntimeConfig` snapshot
/// plus the session's project plugin snapshot. Used at session bootstrap;
/// `/hooks reload` rebuilds from disk separately so plugin edits are visible
/// without restarting. Settings sources are loaded in lowest-precedence-first
/// order so the registry vec uses deterministic iteration order.
pub(super) fn populate_hook_registry(
    registry: &coco_hooks::HookRegistry,
    runtime_config: &coco_config::RuntimeConfig,
    project_services: &ProjectServices,
) {
    let policy = coco_hooks::LoaderPolicy {
        disable_all_hooks: runtime_config.settings.merged.disable_all_hooks,
        allow_managed_hooks_only: runtime_config.settings.merged.allow_managed_hooks_only,
    };
    for source in [
        coco_config::SettingSource::User,
        coco_config::SettingSource::Project,
        coco_config::SettingSource::Local,
        coco_config::SettingSource::Flag,
        coco_config::SettingSource::Policy,
    ] {
        let Some(value) = runtime_config.settings.per_source.get(&source) else {
            continue;
        };
        let Some(hooks_value) = value.get("hooks") else {
            continue;
        };
        let scope = match source {
            coco_config::SettingSource::User => coco_types::HookScope::User,
            coco_config::SettingSource::Project => coco_types::HookScope::Project,
            coco_config::SettingSource::Local => coco_types::HookScope::Local,
            // Flag is treated as Local — closest to the user's
            // explicit per-invocation override.
            coco_config::SettingSource::Flag => coco_types::HookScope::Local,
            coco_config::SettingSource::Policy => coco_types::HookScope::Policy,
            coco_config::SettingSource::Plugin => coco_types::HookScope::Plugin,
        };
        match coco_hooks::load_hooks_from_config_with_policy(hooks_value, scope, policy) {
            Ok(definitions) => {
                for def in definitions {
                    registry.register_deduped(def);
                }
            }
            Err(e) => {
                warn!(error = %e, source = %source, "failed to load hooks from settings — source skipped");
            }
        }
    }
    // Plugin hooks: load the full ENABLED plugin set via the unified V2
    // orchestrator (marketplace versioned cache + local `inline` dirs, gated by
    // settings.json `enabled_plugins`) — not just the local-dir, all-enabled V1
    // scan. `register_plugin_hooks_v2` uses `register_deduped` so a plugin
    // re-declaring a settings hook stays single-fire.
    let plugin_count = project_services.register_plugin_hooks(registry);
    if plugin_count > 0 {
        info!(
            plugins = plugin_count,
            "loaded {} enabled plugin(s)", plugin_count
        );
    }
}

/// Map a coco-config-reload [`TrackedKind`] to the `ConfigChangeSource`
/// wire string consumed by the `ConfigChange` hook. Catalog files
/// (`providers.json`, `models.json`) live alongside the user settings
/// in `config home/`, so they share the `user_settings` source.
/// `flag_settings` falls back to `user_settings` since there is no
/// flag-settings hook source variant.
fn config_change_source_for_kind(
    kind: coco_config_reload::TrackedKind,
) -> coco_hooks::orchestration::ConfigChangeSource {
    use coco_config::SettingSource;
    use coco_config::WatchedKind;
    use coco_config_reload::TrackedKind;
    use coco_hooks::orchestration::ConfigChangeSource;
    match kind {
        TrackedKind::Settings(WatchedKind::Settings(SettingSource::User)) => {
            ConfigChangeSource::UserSettings
        }
        TrackedKind::Settings(WatchedKind::Settings(SettingSource::Project)) => {
            ConfigChangeSource::ProjectSettings
        }
        TrackedKind::Settings(WatchedKind::Settings(SettingSource::Local)) => {
            ConfigChangeSource::LocalSettings
        }
        TrackedKind::Settings(WatchedKind::Settings(SettingSource::Policy)) => {
            ConfigChangeSource::PolicySettings
        }
        TrackedKind::Settings(WatchedKind::Settings(
            SettingSource::Plugin | SettingSource::Flag,
        ))
        | TrackedKind::Settings(WatchedKind::ProvidersCatalog | WatchedKind::ModelsCatalog)
        | TrackedKind::FlagSettings => ConfigChangeSource::UserSettings,
    }
}

impl FileWatchRegistrationContext {
    pub(super) async fn add_paths(&self, paths: Vec<String>) {
        let path_bufs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let mut slot = self.file_changed_watcher.write().await;
        if slot.is_none() {
            let registry = self.hook_registry.clone();
            let cfg = self.engine_config.read().await.clone();
            let session_id = self.session_id.clone();
            let disable_all_hooks = cfg.disable_all_hooks;
            let allow_managed_hooks_only = cfg.allow_managed_hooks_only;
            let project_dir = cfg.project_dir.clone();
            let cwd = cfg.workspace_cwd();
            let cancel = self.cancel.clone();
            let async_registry = self.async_hook_registry.clone();
            let rewake_sink = async_rewake_sink(&self.command_queue);
            let llm_handle = self.hook_llm_handle.clone();
            let factory: Arc<
                dyn Fn() -> coco_hooks::orchestration::OrchestrationContext + Send + Sync,
            > = Arc::new(move || coco_hooks::orchestration::OrchestrationContext {
                session_id: session_id.clone(),
                cwd: cwd.clone(),
                project_dir: project_dir.clone(),
                permission_mode: None,
                transcript_path: None,
                agent_id: None,
                agent_type: None,
                cancel: cancel.clone(),
                disable_all_hooks,
                allow_managed_hooks_only,
                attachment_emitter: coco_messages::AttachmentEmitter::noop(),
                sync_event_sink: None,
                http_url_allowlist: None,
                http_env_var_policy: None,
                async_registry: Some(async_registry.clone()),
                async_rewake_sink: Some(rewake_sink.clone()),
                llm_handle: Some(llm_handle.clone()),
                workspace_trust_accepted: None,
            });
            *slot = crate::file_changed_watcher::FileChangedHookWatcher::new(registry, factory);
        }
        if let Some(watcher) = slot.as_ref() {
            watcher.add_paths(path_bufs);
        }
    }
}

impl SessionRuntime {
    pub fn set_client_hook_callback(&self, callback: coco_hooks::ClientHookCallback) {
        self.hook_resources
            .registry()
            .set_client_hook_callback(callback);
    }

    pub fn register_hook_definitions<I>(&self, hooks: I) -> usize
    where
        I: IntoIterator<Item = coco_hooks::HookDefinition>,
    {
        let registry = self.hook_resources.registry();
        hooks.into_iter().fold(0, |count, hook| {
            count + usize::from(registry.register_deduped(hook))
        })
    }

    pub async fn wrap_send_elicitation_with_hooks(
        self: &Arc<Self>,
        server_name: String,
        base: coco_mcp::SendElicitation,
    ) -> coco_mcp::SendElicitation {
        let elicit_counter = self
            .engine_state_resources
            .app_state()
            .read()
            .await
            .elicitation_pending_count
            .clone();
        crate::elicitation_hooks::wrap_send_elicitation_with_hooks(
            server_name,
            self.hook_resources.registry(),
            self.orchestration_ctx_factory(),
            Some(elicit_counter),
            base,
        )
    }

    /// Fire SessionStart hooks for the given source. The result is buffered
    /// into `sync_hook_buffer` to surface as reminders on the next turn.
    /// Runners call this once at session bootstrap (TUI / AppServer) so the
    /// first turn's reminder pass picks up the events. Failure is
    /// logged + tolerated; no panic on hook misconfig.
    pub async fn fire_session_start_hooks(
        &self,
        source: coco_hooks::orchestration::SessionStartSource,
    ) {
        let cfg = self.current_engine_config().await;
        let session_id = self.current_typed_session_id().await;
        let ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id,
            cwd: cfg.workspace_cwd(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: None,
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.lifecycle_resources.cancel(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            sync_event_sink: Some(self.hook_resources.sync_buffer()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.hook_resources.async_registry()),
            async_rewake_sink: Some(async_rewake_sink(self.command_resources.command_queue())),
            llm_handle: Some(self.hook_resources.llm_handle()),
            workspace_trust_accepted: None,
        };
        let model_arg = if cfg.model_id.is_empty() {
            None
        } else {
            Some(cfg.model_id.as_str())
        };
        let hook_registry = self.hook_resources.registry();
        match coco_hooks::orchestration::execute_session_start(
            &hook_registry,
            &ctx,
            source,
            /*agent_type*/ None,
            model_arg,
        )
        .await
        {
            Ok(agg) => {
                // Hook output may register paths the FileChanged watcher
                // should monitor. Hand them off to the runtime's shared
                // watcher so subsequent file events fire FileChanged hooks.
                // Empty vec is a no-op.
                if !agg.watch_paths.is_empty() {
                    self.add_file_watch_paths(agg.watch_paths.clone()).await;
                }
            }
            Err(e) => {
                warn!(error = %e, ?source, "SessionStart hook execution failed at startup");
            }
        }
    }

    pub async fn fire_session_end_hooks(&self, reason: coco_hooks::orchestration::ExitReason) {
        let cur_session_id = self.current_typed_session_id().await;
        let cfg = self.current_engine_config().await;
        let ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id: cur_session_id,
            cwd: cfg.workspace_cwd(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: None,
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.lifecycle_resources.cancel(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            sync_event_sink: None,
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.hook_resources.async_registry()),
            async_rewake_sink: Some(async_rewake_sink(self.command_resources.command_queue())),
            llm_handle: Some(self.hook_resources.llm_handle()),
            workspace_trust_accepted: None,
        };
        let hook_registry = self.hook_resources.registry();
        if let Err(e) =
            coco_hooks::orchestration::execute_session_end(&hook_registry, &ctx, reason).await
        {
            warn!(error = %e, ?reason, "SessionEnd hook execution failed");
        }
    }

    /// Fire Setup hooks (`Maintenance` at bootstrap, `Init` at `coco init`).
    /// Output is fire-and-forget — Setup is observability-only (no blocking,
    /// no continuation signals). Failure is logged.
    pub async fn fire_setup_hooks(&self, trigger: coco_hooks::orchestration::SetupTrigger) {
        let cfg = self.current_engine_config().await;
        let session_id = self.current_typed_session_id().await;
        let ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id,
            cwd: cfg.workspace_cwd(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: None,
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.lifecycle_resources.cancel(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            sync_event_sink: Some(self.hook_resources.sync_buffer()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.hook_resources.async_registry()),
            async_rewake_sink: Some(async_rewake_sink(self.command_resources.command_queue())),
            llm_handle: Some(self.hook_resources.llm_handle()),
            workspace_trust_accepted: None,
        };
        let hook_registry = self.hook_resources.registry();
        if let Err(e) =
            coco_hooks::orchestration::execute_setup(&hook_registry, &ctx, trigger).await
        {
            warn!(error = %e, ?trigger, "Setup hook execution failed");
        }
    }

    /// Fire UserPromptSubmit hooks for the given prompt text. Output
    /// flows into the shared `sync_hook_buffer`. Returns the aggregated
    /// result so the caller can honour `blocking_error` (suppress the
    /// turn) and `prevent_continuation` (skip the turn but keep the
    /// prompt).
    pub async fn fire_user_prompt_submit_hooks(
        &self,
        prompt: &str,
    ) -> coco_hooks::orchestration::AggregatedHookResult {
        let cfg = self.current_engine_config().await;
        let session_id = self.current_typed_session_id().await;
        let ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id,
            cwd: cfg.workspace_cwd(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: Some(format!("{:?}", cfg.permission_mode)),
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.lifecycle_resources.cancel(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            sync_event_sink: Some(self.hook_resources.sync_buffer()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.hook_resources.async_registry()),
            async_rewake_sink: Some(async_rewake_sink(self.command_resources.command_queue())),
            llm_handle: Some(self.hook_resources.llm_handle()),
            workspace_trust_accepted: None,
        };
        let hook_registry = self.hook_resources.registry();
        match coco_hooks::orchestration::execute_user_prompt_submit(&hook_registry, &ctx, prompt)
            .await
        {
            Ok(agg) => agg,
            Err(e) => {
                warn!(error = %e, "UserPromptSubmit hook execution failed");
                coco_hooks::orchestration::AggregatedHookResult::default()
            }
        }
    }

    /// Fire Notification hooks. Called from `TuiPermissionBridge` /
    /// `AppServerPermissionBridge` when the user is about to be asked for
    /// input (`permission_prompt`), and from any future idle / elicitation
    /// prompts. Output is fire-and-forget — awaited only to preserve
    /// ordering before the actual UI notification, never to block the
    /// prompt itself.
    pub async fn fire_notification_hooks(
        &self,
        notification_type: &str,
        message: &str,
        title: Option<&str>,
    ) {
        let cfg = self.current_engine_config().await;
        let session_id = self.current_typed_session_id().await;
        let ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id,
            cwd: cfg.workspace_cwd(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: Some(format!("{:?}", cfg.permission_mode)),
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.lifecycle_resources.cancel(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            sync_event_sink: Some(self.hook_resources.sync_buffer()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.hook_resources.async_registry()),
            async_rewake_sink: Some(async_rewake_sink(self.command_resources.command_queue())),
            llm_handle: Some(self.hook_resources.llm_handle()),
            workspace_trust_accepted: None,
        };
        let hook_registry = self.hook_resources.registry();
        if let Err(e) = coco_hooks::orchestration::execute_notification(
            &hook_registry,
            &ctx,
            notification_type,
            message,
            title,
        )
        .await
        {
            warn!(
                error = %e,
                notification_type,
                "Notification hook execution failed"
            );
        }
    }

    pub(super) fn file_watch_registration_context(&self) -> FileWatchRegistrationContext {
        FileWatchRegistrationContext {
            file_changed_watcher: self.hook_resources.file_changed_watcher(),
            hook_registry: self.hook_resources.registry(),
            session_id: self.engine_config_resources.session_id().clone(),
            engine_config: self.engine_config_resources.engine_config().clone(),
            cancel: self.lifecycle_resources.cancel(),
            async_hook_registry: self.hook_resources.async_registry(),
            command_queue: self.command_resources.command_queue().clone(),
            hook_llm_handle: self.hook_resources.llm_handle(),
        }
    }

    /// Append paths to the `FileChanged` watcher, lazily constructing
    /// it on first call. Empty input is a no-op.
    pub async fn add_file_watch_paths(&self, paths: Vec<String>) {
        if paths.is_empty() {
            return;
        }
        self.file_watch_registration_context()
            .add_paths(paths)
            .await;
    }

    /// Fire CwdChanged hooks.
    /// Callers must capture the old cwd before mutating
    /// `std::env::current_dir`. Surfacing the helper lets ad-hoc
    /// cwd-mutating code paths (worktree exit, AppServer setCwd control) wire
    /// the hook without re-implementing the orchestration context build.
    pub async fn fire_cwd_changed_hooks(&self, old_cwd: &str, new_cwd: &str) {
        let cfg = self.current_engine_config().await;
        let session_id = self.current_typed_session_id().await;
        let ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id,
            cwd: std::path::PathBuf::from(new_cwd),
            project_dir: cfg.project_dir.clone(),
            permission_mode: Some(format!("{:?}", cfg.permission_mode)),
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.lifecycle_resources.cancel(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            sync_event_sink: Some(self.hook_resources.sync_buffer()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.hook_resources.async_registry()),
            async_rewake_sink: Some(async_rewake_sink(self.command_resources.command_queue())),
            llm_handle: Some(self.hook_resources.llm_handle()),
            workspace_trust_accepted: None,
        };
        let hook_registry = self.hook_resources.registry();
        match coco_hooks::orchestration::execute_cwd_changed(&hook_registry, &ctx, old_cwd, new_cwd)
            .await
        {
            Ok(agg) => {
                // The cwd swap is a natural moment for hooks to update
                // the FileChanged watch list (e.g. add the new project's
                // `.envrc`).
                if !agg.watch_paths.is_empty() {
                    self.add_file_watch_paths(agg.watch_paths.clone()).await;
                }
            }
            Err(e) => {
                warn!(error = %e, old_cwd, new_cwd, "CwdChanged hook execution failed");
            }
        }
    }

    /// Fire ConfigChange hooks.
    pub async fn run_config_change_hooks(
        &self,
        source: coco_hooks::orchestration::ConfigChangeSource,
        file_path: Option<&str>,
    ) -> coco_hooks::orchestration::AggregatedHookResult {
        let cfg = self.current_engine_config().await;
        let session_id = self.current_typed_session_id().await;
        let ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id,
            cwd: cfg.workspace_cwd(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: Some(format!("{:?}", cfg.permission_mode)),
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.lifecycle_resources.cancel(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            sync_event_sink: Some(self.hook_resources.sync_buffer()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.hook_resources.async_registry()),
            async_rewake_sink: Some(async_rewake_sink(self.command_resources.command_queue())),
            llm_handle: Some(self.hook_resources.llm_handle()),
            workspace_trust_accepted: None,
        };
        let hook_registry = self.hook_resources.registry();
        match coco_hooks::orchestration::execute_config_change(
            &hook_registry,
            &ctx,
            source,
            file_path,
        )
        .await
        {
            Ok(agg) => agg,
            Err(e) => {
                warn!(error = %e, source = ?source, "ConfigChange hook execution failed");
                coco_hooks::orchestration::AggregatedHookResult::default()
            }
        }
    }

    /// Fire ConfigChange hooks for observe-only reload pipelines.
    pub async fn fire_config_change_hooks(
        &self,
        source: coco_hooks::orchestration::ConfigChangeSource,
        file_path: Option<&str>,
    ) {
        let _ = self.run_config_change_hooks(source, file_path).await;
    }

    /// Spawn a tokio task that subscribes to a [`coco_config_reload::ConfigChange`]
    /// stream and fires the corresponding `ConfigChange` hook for each event.
    /// Returns the [`tokio::task::JoinHandle`] so the caller can hold it for
    /// the session lifetime; dropping it aborts the watcher.
    /// `cancel` lets callers terminate the watcher proactively
    /// (typically the session-level shutdown token); when the
    /// broadcast channel closes (reloader dropped), the loop exits on
    /// its own.
    pub fn spawn_config_change_watcher(
        self: &Arc<Self>,
        mut rx: tokio::sync::broadcast::Receiver<coco_config_reload::ConfigChange>,
    ) -> tokio::task::JoinHandle<()> {
        let runtime = Arc::clone(self);
        let cancel = self.lifecycle_resources.cancel();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    recv = rx.recv() => match recv {
                        Ok(change) => {
                            let source = config_change_source_for_kind(change.kind);
                            let path = change.path.to_string_lossy().into_owned();
                            runtime
                                .fire_config_change_hooks(source, Some(&path))
                                .await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            warn!(skipped, "ConfigChange watcher lagged; events dropped");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        })
    }
}
