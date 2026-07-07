use coco_types::SessionId;
use tracing::warn;

use super::SessionRuntime;

#[derive(Clone, Copy)]
enum RetargetUsageLedger {
    Empty,
    Loaded,
}

impl SessionRuntime {
    /// Retarget the fused runtime to a newly-created empty SDK session.
    ///
    /// This is the temporary pre-registry equivalent of `session/create`:
    /// identity-bearing boundaries move to `new_session_id`, and runtime-owned
    /// session-scoped caches start empty under that id.
    pub async fn retarget_for_new_session(&self, new_session_id: SessionId) {
        self.retarget_runtime_for_session(&new_session_id, RetargetUsageLedger::Empty)
            .await;
    }

    /// Retarget the fused runtime to an SDK/TUI-loaded session.
    ///
    /// Caller-owned state (`SessionHandle.history`,
    /// `SessionHandle.app_state` per the SDK protocol) is created fresh
    /// by the caller; this method only refreshes runtime-owned state
    /// keyed on session_id.
    ///
    /// What stays:
    /// - `hook_registry`, `tools`, `client` (and Arc identity), other
    /// process-level resources — these are correctly cross-session.
    ///
    /// This is a temporary compatibility seam for the pre-registry runtime:
    /// SDK `session/resume` and TUI `/resume` still reuse one
    /// `SessionRuntime` and repoint its session-keyed internals.
    /// A real multi-session runtime will create/load a distinct runtime
    /// instead of calling this method.
    ///
    /// Distinct from `clear_conversation`: that fires SessionEnd /
    /// SessionStart hooks and performs the full `/clear` flow. This method
    /// skips both — SDK `session/archive` / resume orchestration owns those
    /// lifecycle boundaries.
    pub async fn retarget_for_loaded_session(&self, new_session_id: SessionId) {
        self.retarget_runtime_for_session(&new_session_id, RetargetUsageLedger::Loaded)
            .await;
    }

    async fn retarget_runtime_for_session(
        &self,
        new_session_id: &SessionId,
        usage_ledger: RetargetUsageLedger,
    ) {
        // This compatibility path becomes runtime creation/load once the
        // registry lands. Keep every in-place session-keyed mutation here so
        // the runtime split has one demolition point.
        self.flush_session_usage_snapshot().await;
        self.retarget_session_id_boundaries(new_session_id).await;
        self.reset_todo_list().await;
        match usage_ledger {
            RetargetUsageLedger::Empty => {
                self.usage_accounting
                    .retarget_to_empty_session(new_session_id.clone())
                    .await;
            }
            RetargetUsageLedger::Loaded => {
                self.usage_accounting
                    .retarget_to_loaded_session(new_session_id.clone())
                    .await;
            }
        }
        self.reset_runtime_state_after_session_retarget().await;
    }

    async fn reset_runtime_state_after_session_retarget(&self) {
        {
            let mut frs = self.file_read_state.write().await;
            frs.clear();
        }
        if let Some(sandbox) = &self.sandbox_state {
            sandbox.clear_session_allowed_hosts();
        }
        self.denial_tracker.lock().await.clear();
        *self.tool_result_replacement_state.write().await =
            coco_tool_runtime::tool_result_storage::ContentReplacementState::new(i64::MAX);
        self.transcript_dedup.lock().await.clear();
        self.reset_cache_break_detectors().await;
    }

    /// Repoint session-id-keyed runtime boundaries at `new_session_id`.
    ///
    /// This is the legacy compatibility shim while loaded-session flows and
    /// `/clear` still retarget the fused runtime in place. It is deliberately
    /// limited to identity-bearing runtime services whose state is retargeted
    /// without changing tracker contents; callers own broader cache / history
    /// reset semantics. File-history persistence derives its session id from
    /// the synchronized engine-config mirror and does not retarget separately.
    async fn retarget_session_id_boundaries(&self, new_session_id: &SessionId) {
        self.retarget_engine_config_session_id(new_session_id).await;
        self.retarget_memory_session_id(new_session_id).await;
        self.retarget_model_runtime_session_id(new_session_id);
    }

    async fn retarget_engine_config_session_id(&self, new_session_id: &SessionId) {
        let new_id_for_cfg = new_session_id.clone();
        self.update_engine_config(|cfg| cfg.session_id = new_id_for_cfg)
            .await;
    }

    async fn retarget_memory_session_id(&self, new_session_id: &SessionId) {
        if let Some(runtime) = &self.memory_runtime {
            runtime.set_session_id(new_session_id.clone()).await;
        }
    }

    fn retarget_model_runtime_session_id(&self, new_session_id: &SessionId) {
        // Refresh `${SESSION_ID}` (and other session-scoped header-template
        // vars) so templated provider headers re-expand against the new id and
        // the affected clients rebuild. Without this, the model-runtime
        // registry keeps the bootstrap id baked into every client and a
        // `/clear` / `/resume` regen sends a stale session id to the gateway.
        if let Err(e) = self.model_runtimes.update_session_id(new_session_id) {
            warn!(error = %e, "failed to refresh model-runtime header vars after session-id change");
        }
    }
}
