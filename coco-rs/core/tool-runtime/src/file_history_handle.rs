//! Checkpoint/rewind edit-tracking seam.
//!
//! File-mutating tools must record every edit before touching disk so the
//! session can rewind. The store itself (`coco_context::FileHistoryState`)
//! owns snapshot files, backup directories, and diff rendering — a whole
//! subsystem this crate has no business linking against just to carry a
//! handle to it. Tools need exactly one operation, so that is all the
//! trait exposes; the app layer adapts its concrete store onto it.

use std::path::Path;
use std::sync::Arc;

/// Records a pre-edit snapshot of `file_path` for checkpoint/rewind.
#[async_trait::async_trait]
pub trait FileHistoryHandle: Send + Sync {
    /// `message_id` is the originating **user** message id, not the
    /// tool_use id — a rewind restores to a user turn, and several tool
    /// calls can share one.
    ///
    /// Returns the store's own error text. Tracking is best-effort: the
    /// caller warns and proceeds, because losing a rewind point must not
    /// fail the edit the model asked for.
    async fn track_edit(
        &self,
        file_path: &Path,
        message_id: &str,
        session_id: &str,
    ) -> Result<(), String>;
}

pub type FileHistoryHandleRef = Arc<dyn FileHistoryHandle>;

/// Used by sessions that never wired a history store (tests, minimal SDK
/// embeddings). Edits still apply; they are simply not rewindable.
#[derive(Debug, Default)]
pub struct NoOpFileHistoryHandle;

#[async_trait::async_trait]
impl FileHistoryHandle for NoOpFileHistoryHandle {
    async fn track_edit(&self, _: &Path, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
}
