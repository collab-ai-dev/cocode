//! Concrete `CompactionObserver` implementations.
//!
//! The compact crate provides only the `CompactionObserver` trait + registry;
//! actual cache-invalidation logic lives here so the engine can register
//! observers from the same crate that owns `Arc<RwLock<…>>` handles for
//! file state, permission denial caches, and skill state.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tracing::warn;

use coco_compact::{CompactResult, CompactionObserver};
use coco_error::BoxedError;
use coco_messages::Message;

/// Observer that drops `FileReadState` LRU entries after a successful
/// compaction. The engine already snapshots+clears synchronously in
/// `try_full_compact` step 2; this observer covers the SM-first path
/// where the engine only `clear()`s but doesn't snapshot — and any
/// future strategies that bypass the LLM path entirely.
pub struct FileReadStateObserver {
    file_read_state: Arc<RwLock<coco_context::FileReadState>>,
}

impl FileReadStateObserver {
    pub fn new(file_read_state: Arc<RwLock<coco_context::FileReadState>>) -> Self {
        Self { file_read_state }
    }
}

#[async_trait]
impl CompactionObserver for FileReadStateObserver {
    async fn on_compaction_complete(
        &self,
        _result: &CompactResult,
        _is_main_agent: bool,
    ) -> Result<(), BoxedError> {
        let mut frs = self.file_read_state.write().await;
        // Don't fully clear — `try_full_compact`'s snapshot+clear has
        // already done so for the LLM path. We just defensively clear
        // again so SM-first / time-based paths can't leak stale entries.
        frs.clear();
        Ok(())
    }
}

/// Observer that drops the permission `DenialTracker` history after a
/// compaction. Without this, denials from pre-compact tool calls keep
/// counting against the killswitch even though their conversational
/// context is gone.
pub struct ApprovalsObserver {
    denial_tracker: Arc<Mutex<coco_permissions::DenialTracker>>,
}

impl ApprovalsObserver {
    pub fn new(denial_tracker: Arc<Mutex<coco_permissions::DenialTracker>>) -> Self {
        Self { denial_tracker }
    }
}

#[async_trait]
impl CompactionObserver for ApprovalsObserver {
    async fn on_compaction_complete(
        &self,
        _result: &CompactResult,
        is_main_agent: bool,
    ) -> Result<(), BoxedError> {
        if !is_main_agent {
            return Ok(());
        }
        let mut tracker = self.denial_tracker.lock().await;
        let pre_total = tracker.total_denials;
        tracker.clear();
        if pre_total > 0 {
            warn!(
                cleared = pre_total,
                "ApprovalsObserver: cleared DenialTracker after compact"
            );
        }
        Ok(())
    }
}

/// Observer that resets ephemeral cross-turn `ToolAppState` counters
/// after compaction (e.g. `needs_plan_mode_exit_attachment`,
/// `awaiting_plan_approval_request_id`). The transcript past those
/// flags is gone, so leaving them set would re-fire reminders that
/// targeted now-archived turns.
pub struct ToolAppStateObserver {
    app_state: Arc<RwLock<coco_types::ToolAppState>>,
}

impl ToolAppStateObserver {
    pub fn new(app_state: Arc<RwLock<coco_types::ToolAppState>>) -> Self {
        Self { app_state }
    }
}

#[async_trait]
impl CompactionObserver for ToolAppStateObserver {
    async fn on_compaction_complete(
        &self,
        _result: &CompactResult,
        is_main_agent: bool,
    ) -> Result<(), BoxedError> {
        if !is_main_agent {
            return Ok(());
        }
        let mut guard = self.app_state.write().await;
        guard.needs_plan_mode_exit_attachment = false;
        guard.awaiting_plan_approval_request_id = None;
        Ok(())
    }

    async fn on_post_compact(&self, _new_messages: &[Message]) -> Result<(), BoxedError> {
        Ok(())
    }
}

/// Convenience builder — assemble a registry with the standard
/// observers. Callers (CLI/SDK runners) feed in the handles they
/// own; missing handles map to omitted observers.
pub fn build_default_registry(
    file_read_state: Option<Arc<RwLock<coco_context::FileReadState>>>,
    denial_tracker: Option<Arc<Mutex<coco_permissions::DenialTracker>>>,
    app_state: Option<Arc<RwLock<coco_types::ToolAppState>>>,
) -> Arc<coco_compact::CompactionObserverRegistry> {
    let mut registry = coco_compact::CompactionObserverRegistry::new();
    if let Some(frs) = file_read_state {
        registry.register(Arc::new(FileReadStateObserver::new(frs)));
    }
    if let Some(dt) = denial_tracker {
        registry.register(Arc::new(ApprovalsObserver::new(dt)));
    }
    if let Some(app) = app_state {
        registry.register(Arc::new(ToolAppStateObserver::new(app)));
    }
    Arc::new(registry)
}
