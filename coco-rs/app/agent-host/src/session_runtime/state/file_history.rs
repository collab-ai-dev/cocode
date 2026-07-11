use std::sync::Arc;

use coco_query::QueryEngineConfig;
use tracing::warn;

use crate::session_runtime::clone_std_rwlock;

/// `FileHistorySnapshotSink` that writes via [`coco_session::TranscriptStore`].
/// Lives here because both runners need to install it on `FileHistoryState`.
/// **Deliberately bypasses the `SessionStore` backend trait** and constructs a
/// concrete disk store directly: file-history checkpoints are a low-frequency,
/// local-only convenience (rewind / disk backup), not authoritative session
/// state - they are not needed to recover a conversation and are explicitly
/// declared local-cache (`docs/coco-rs/session-storage-backend-design.md`
/// §3.4). So even under a non-`Disk` `session.backend` they stay on disk;
/// there's nothing to gain from routing them through the swappable boundary.
/// Reads the current session id from the synchronized engine-config mirror so
/// file-history does not need a separate mutable identity mirror.
pub(in crate::session_runtime) struct TranscriptFileHistorySink {
    store: coco_session::TranscriptStore,
    engine_config: Arc<std::sync::RwLock<QueryEngineConfig>>,
}

impl TranscriptFileHistorySink {
    pub(in crate::session_runtime) fn new(
        project_paths: Arc<coco_paths::ProjectPaths>,
        engine_config: Arc<std::sync::RwLock<QueryEngineConfig>>,
    ) -> Self {
        Self {
            store: coco_session::TranscriptStore::new(project_paths),
            engine_config,
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
        let id = clone_std_rwlock(&self.engine_config).session_id.to_string();
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
/// (SDK / headless) default OFF and require the SDK-enable env. The
/// disable env always wins.
pub(in crate::session_runtime) fn file_checkpointing_enabled(
    settings_enabled: bool,
    is_non_interactive: bool,
) -> bool {
    if coco_config::env::is_env_truthy(coco_config::EnvKey::CocoFileCheckpointingDisable) {
        return false;
    }
    if is_non_interactive {
        coco_config::env::is_env_truthy(coco_config::EnvKey::CocoFileCheckpointingSdkEnable)
    } else {
        settings_enabled
    }
}
