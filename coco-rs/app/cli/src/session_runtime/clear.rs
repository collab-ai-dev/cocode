use anyhow::Result;
use coco_messages::Message;
use coco_types::SessionId;
use tracing::warn;

use super::SessionRuntime;
use super::hooks::async_rewake_sink;

impl SessionRuntime {
    async fn capture_pre_clear_rewind_messages(&self) {
        let pre_clear_messages = self.history.lock().await.as_slice().to_vec();
        *self.clear_rewind_messages.lock().await = pre_clear_messages
            .iter()
            .any(|m| matches!(m.as_ref(), Message::User(_)))
            .then_some(pre_clear_messages);
    }

    async fn reset_conversation_state_before_clear_retarget(&self) {
        // `/clear` clears the conversation, not the session's permission
        // grants. Preserve the live permission base and reset only
        // conversation-local latches / todos / snapshots.
        {
            let mut guard = self.app_state.write().await;
            let preserved = std::mem::take(&mut guard.permissions);
            *guard = coco_types::ToolAppState::default();
            guard.permissions = preserved;
        }
        self.reset_todo_list().await;
        self.reset_cache_break_detectors().await;
        // Drop the captured post-turn cache-safe-params handle. Otherwise a
        // `/btw` issued between this `/clear` and the first post-clear turn
        // would read the dropped pre-clear engine's slot (still holding the
        // discarded conversation in `fork_context_messages`) and fork against
        // content the user just cleared. With the handle nulled, `/btw` falls
        // back to fresh cache params built from the now-empty transcript.
        *self.last_engine_cache_handle.write().await = None;
        self.skill_manager.reset_announcements();
        self.command_queue.clear().await;

        let cur_session_id = self.current_typed_session_id().await;
        coco_context::clear_plan_slug(cur_session_id.as_str());
        {
            let mut frs = self.file_read_state.write().await;
            frs.clear();
        }
        if let Some(fh) = &self.file_history {
            let mut fh = fh.write().await;
            *fh = coco_context::FileHistoryState::default();
        }

        // Reset auto-memory per-conversation state. The on-disk MEMORY.md and
        // topic files are cross-conversation memory and intentionally survive.
        if let Some(runtime) = &self.memory_runtime {
            runtime.reset().await;
        }
    }

    async fn run_session_start_hooks_after_clear(&self, new_session_id: SessionId) {
        let cfg = self.current_engine_config().await;
        let post_ctx = coco_hooks::orchestration::OrchestrationContext {
            session_id: new_session_id,
            cwd: cfg.workspace_cwd(),
            project_dir: cfg.project_dir.clone(),
            permission_mode: None,
            transcript_path: None,
            agent_id: None,
            agent_type: None,
            cancel: self.cancel.clone(),
            disable_all_hooks: cfg.disable_all_hooks,
            allow_managed_hooks_only: cfg.allow_managed_hooks_only,
            attachment_emitter: coco_messages::AttachmentEmitter::noop(),
            // Surface SessionStart hook output as `hook_*` reminders on
            // the next turn.
            sync_event_sink: Some(self.sync_hook_buffer.clone()),
            http_url_allowlist: None,
            http_env_var_policy: None,
            async_registry: Some(self.async_hook_registry.clone()),
            async_rewake_sink: Some(async_rewake_sink(&self.command_queue)),
            llm_handle: Some(self.hook_llm_handle.clone()),
            workspace_trust_accepted: None,
        };
        let model_arg = if cfg.model_id.is_empty() {
            None
        } else {
            Some(cfg.model_id.as_str())
        };

        // Clear the in-memory transcript before invoking SessionStart hooks.
        // The hook output flows into the sync hook buffer and surfaces as
        // `hook_*` reminders on the next turn.
        {
            let mut h = self.history.lock().await;
            h.clear();
        }
        if let Err(e) = coco_hooks::orchestration::execute_session_start(
            &self.hook_registry,
            &post_ctx,
            coco_hooks::orchestration::SessionStartSource::Clear,
            /*agent_type*/ None,
            model_arg,
        )
        .await
        {
            warn!(error = %e, "SessionStart hook execution failed during /clear");
        }
    }

    /// Full `/clear` reset: SessionEnd hooks → drop subsystem caches →
    /// regen session id → SessionStart hooks (whose result messages seed
    /// the new transcript).
    /// `/clear` has one behavior: full reset, regardless of arguments.
    pub async fn clear_conversation(&self) -> Result<()> {
        // Step 1: SessionEnd hooks fire BEFORE the reset, with the bounded
        // SESSION_END timeout (1.5s default;
        // `COCO_SESSIONEND_HOOKS_TIMEOUT_MS` overrides).
        self.fire_session_end_hooks(coco_hooks::orchestration::ExitReason::Clear)
            .await;

        // Step 2: capture a rewind prefix before dropping conversation state.
        self.capture_pre_clear_rewind_messages().await;

        // Step 3: reset conversation-local caches before the session id moves.
        self.reset_conversation_state_before_clear_retarget().await;

        // Step 4: regenerate the session id and propagate it through the
        // same empty-session compatibility seam as SDK `session/start`.
        let new_session_id = SessionId::generate();
        self.retarget_for_new_session(new_session_id.clone()).await;

        // Step 5: SessionStart hooks. Result messages seed the post-clear transcript.
        self.run_session_start_hooks_after_clear(new_session_id)
            .await;

        Ok(())
    }
}
