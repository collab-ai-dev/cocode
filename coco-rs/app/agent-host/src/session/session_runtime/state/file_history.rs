use std::sync::Arc;

use tracing::warn;

/// `FileHistorySnapshotSink` that writes via [`coco_session::TranscriptStore`].
/// Lives here because both runners need to install it on `FileHistoryState`.
/// **Deliberately bypasses the `SessionStore` backend trait** and constructs a
/// concrete disk store directly: file-history checkpoints are a low-frequency,
/// local-only convenience (rewind / disk backup), not authoritative session
/// state - they are not needed to recover a conversation and are explicitly
/// declared local-cache (`docs/coco-rs/session-storage-backend-design.md`
/// §3.4). So even under a non-`Disk` `session.backend` they stay on disk;
/// there's nothing to gain from routing them through the swappable boundary.
/// Holds the immutable session id directly, so file-history persistence does
/// not depend on the mutable engine-config surface.
pub(in crate::session::session_runtime) struct TranscriptFileHistorySink {
    store: coco_session::TranscriptStore,
    session_id: coco_types::SessionId,
}

impl TranscriptFileHistorySink {
    pub(in crate::session::session_runtime) fn new(
        project_paths: Arc<coco_paths::ProjectPaths>,
        session_id: coco_types::SessionId,
    ) -> Self {
        Self {
            store: coco_session::TranscriptStore::new(project_paths),
            session_id,
        }
    }
}

#[async_trait::async_trait]
impl coco_context::FileHistorySnapshotSink for TranscriptFileHistorySink {
    async fn record(
        &self,
        message_id: &str,
        snapshot_json: serde_json::Value,
        is_snapshot_update: bool,
    ) {
        let id = self.session_id.to_string();
        if let Err(e) = self.store.insert_file_history_snapshot(
            &id,
            message_id,
            snapshot_json,
            is_snapshot_update,
        ) {
            warn!(error = %e, message_id, "failed to persist file-history snapshot");
        }
    }
}

/// File-history checkpointing gate. Interactive sessions default ON
/// (settings flag, unless the disable env is set); non-interactive
/// sessions default OFF and require the noninteractive-enable env. The disable
/// env always wins.
pub(in crate::session::session_runtime) fn file_checkpointing_enabled(
    settings_enabled: bool,
    is_non_interactive: bool,
) -> bool {
    if coco_config::env::is_env_truthy(coco_config::EnvKey::CocoFileCheckpointingDisable) {
        return false;
    }
    if is_non_interactive {
        coco_config::env::is_env_truthy(
            coco_config::EnvKey::CocoFileCheckpointingNoninteractiveEnable,
        )
    } else {
        settings_enabled
    }
}
