use super::super::hooks::async_rewake_sink;
use super::*;

impl SessionRuntime {
    /// Best-effort PID-registry live patch - push the human-readable
    /// session name into the `<config_home>/sessions/<pid>.json` file
    /// that `coco ps` reads. Silently no-ops when the session isn't
    /// registered (subagent context, FS-constrained startup, etc.)
    /// because the PID registry guard is optional. Without this patch the live
    /// registry shows the stale startup name forever.
    pub fn update_session_registry_name(&self, name: &str) {
        self.lifecycle_resources.update_session_registry_name(name);
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
    /// Spawn a session-owned background task and retain its handle so close can
    /// join it (CS-3 session task supervisor). Use this instead of raw
    /// `tokio::spawn` for tasks that must not outlive the session.
    pub(crate) fn spawn_session_task<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.lifecycle_resources.track_task(tokio::spawn(future));
    }
    /// Abort and join every retained session-owned task under one deadline.
    /// Callers cancel the shutdown signal first so cooperative tasks exit early.
    pub(crate) async fn join_session_tasks(&self, deadline: tokio::time::Instant) {
        self.lifecycle_resources.join_tasks(deadline).await;
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
                session_id: runtime.engine_config_resources.session_id().clone(),
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
}
