//! Checkpoint and resume for batch indexing.
//!
//! Tracks indexing progress to allow resumption after interruption.
//! Reference: Continue `core/indexing/refreshIndex.ts`, Tabby `crates/tabby-index/src/indexer.rs`

use std::sync::Arc;

use rusqlite::Connection;
use rusqlite::params;

use crate::error::Result;
use crate::error::RetrievalErr;
use crate::storage::sqlite::OptionalExt;
use crate::storage::sqlite::SqliteStore;

/// Indexing phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexPhase {
    /// Scanning files
    Scanning,
    /// Indexing content
    Indexing,
    /// Committing changes
    Committing,
    /// Completed successfully
    Completed,
    /// Failed with error
    Failed,
}

impl IndexPhase {
    fn as_str(&self) -> &'static str {
        match self {
            IndexPhase::Scanning => "scanning",
            IndexPhase::Indexing => "indexing",
            IndexPhase::Committing => "committing",
            IndexPhase::Completed => "completed",
            IndexPhase::Failed => "failed",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "scanning" => IndexPhase::Scanning,
            "indexing" => IndexPhase::Indexing,
            "committing" => IndexPhase::Committing,
            "completed" => IndexPhase::Completed,
            "failed" => IndexPhase::Failed,
            _ => IndexPhase::Scanning,
        }
    }
}

/// Checkpoint state for resuming interrupted indexing.
#[derive(Debug, Clone)]
pub struct CheckpointState {
    /// Workspace identifier
    pub workspace: String,
    /// Current indexing phase
    pub phase: IndexPhase,
    /// Total files to process
    pub total_files: i32,
    /// Number of files processed
    pub processed_files: i32,
    /// Last successfully processed file path
    pub last_file: Option<String>,
    /// Timestamp when indexing started
    pub started_at: i64,
    /// Timestamp of last update
    pub updated_at: i64,
}

impl CheckpointState {
    /// Check if this checkpoint can be resumed.
    pub fn is_resumable(&self) -> bool {
        matches!(self.phase, IndexPhase::Scanning | IndexPhase::Indexing)
            && self.processed_files < self.total_files
    }

    /// Get progress percentage (0-100).
    pub fn progress_percent(&self) -> i32 {
        if self.total_files == 0 {
            0
        } else {
            (self.processed_files * 100) / self.total_files
        }
    }

    /// Get remaining files count.
    pub fn remaining_files(&self) -> i32 {
        (self.total_files - self.processed_files).max(0)
    }
}

/// Checkpoint manager for tracking indexing progress.
pub struct Checkpoint {
    store: Arc<SqliteStore>,
}

impl Checkpoint {
    /// Create a new checkpoint manager.
    pub fn new(store: Arc<SqliteStore>) -> Self {
        Self { store }
    }

    /// Start a new indexing session.
    ///
    /// Creates or resets the checkpoint for the given workspace.
    pub async fn start(&self, workspace: &str, total_files: i32) -> Result<()> {
        let workspace = workspace.to_string();
        let now = current_timestamp();

        self.store
            .transaction(move |conn| start_checkpoint(conn, &workspace, total_files, now))
            .await
    }

    /// Update progress after processing a file.
    pub async fn update_progress(&self, workspace: &str, last_file: &str) -> Result<()> {
        let workspace = workspace.to_string();
        let last_file = last_file.to_string();
        let now = current_timestamp();

        self.store
            .query(move |conn| {
                conn.execute(
                    "UPDATE checkpoint SET
                        processed_files = processed_files + 1,
                        last_file = ?,
                        updated_at = ?
                     WHERE workspace = ?",
                    params![last_file, now, workspace],
                )
                .map_err(RetrievalErr::from)?;
                Ok(())
            })
            .await
    }

    /// Update progress with batch count.
    pub async fn update_progress_batch(
        &self,
        workspace: &str,
        processed_count: i32,
        last_file: &str,
    ) -> Result<()> {
        let workspace = workspace.to_string();
        let last_file = last_file.to_string();
        let now = current_timestamp();

        self.store
            .query(move |conn| {
                conn.execute(
                    "UPDATE checkpoint SET
                        processed_files = ?,
                        last_file = ?,
                        updated_at = ?
                     WHERE workspace = ?",
                    params![processed_count, last_file, now, workspace],
                )
                .map_err(RetrievalErr::from)?;
                Ok(())
            })
            .await
    }

    /// Set the indexing phase.
    pub async fn set_phase(&self, workspace: &str, phase: IndexPhase) -> Result<()> {
        let workspace = workspace.to_string();
        let phase_str = phase.as_str().to_string();
        let now = current_timestamp();

        self.store
            .query(move |conn| {
                conn.execute(
                    "UPDATE checkpoint SET phase = ?, updated_at = ? WHERE workspace = ?",
                    params![phase_str, now, workspace],
                )
                .map_err(RetrievalErr::from)?;
                Ok(())
            })
            .await
    }

    /// Mark indexing as completed.
    pub async fn complete(&self, workspace: &str) -> Result<()> {
        self.set_phase(workspace, IndexPhase::Completed).await
    }

    /// Mark indexing as failed.
    pub async fn fail(&self, workspace: &str) -> Result<()> {
        self.set_phase(workspace, IndexPhase::Failed).await
    }

    /// Load checkpoint state for a workspace.
    pub async fn load(&self, workspace: &str) -> Result<Option<CheckpointState>> {
        let workspace = workspace.to_string();

        self.store
            .query(move |conn| load_checkpoint(conn, &workspace))
            .await
    }

    /// Check if there is a resumable checkpoint for the workspace.
    pub async fn has_resumable(&self, workspace: &str) -> Result<bool> {
        let state = self.load(workspace).await?;
        Ok(state.map(|s| s.is_resumable()).unwrap_or(false))
    }

    /// Get the last processed file for resume.
    pub async fn get_resume_point(&self, workspace: &str) -> Result<Option<String>> {
        let state = self.load(workspace).await?;
        Ok(state.and_then(|s| if s.is_resumable() { s.last_file } else { None }))
    }

    /// Clear checkpoint for a workspace.
    pub async fn clear(&self, workspace: &str) -> Result<()> {
        let workspace = workspace.to_string();

        self.store
            .query(move |conn| {
                conn.execute(
                    "DELETE FROM checkpoint WHERE workspace = ?",
                    params![workspace],
                )
                .map_err(RetrievalErr::from)?;
                Ok(())
            })
            .await
    }

    /// Get all active checkpoints.
    pub async fn list_active(&self) -> Result<Vec<CheckpointState>> {
        self.store
            .query(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT workspace, phase, total_files, processed_files, last_file, started_at, updated_at
                     FROM checkpoint
                     WHERE phase IN ('scanning', 'indexing')",
                )?;

                let rows = stmt.query_map([], |row| {
                    Ok(CheckpointState {
                        workspace: row.get(0)?,
                        phase: IndexPhase::from_str(&row.get::<_, String>(1)?),
                        total_files: row.get(2)?,
                        processed_files: row.get(3)?,
                        last_file: row.get(4)?,
                        started_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                })?;

                let mut result = Vec::new();
                for row in rows {
                    result.push(row?);
                }
                Ok(result)
            })
            .await
    }
}

/// Stale threshold in seconds - if a checkpoint hasn't been updated in this time,
/// it's considered abandoned and can be safely overwritten.
const STALE_THRESHOLD_SECS: i64 = 300; // 5 minutes

fn start_checkpoint(conn: &Connection, workspace: &str, total_files: i32, now: i64) -> Result<()> {
    // First, check if there's an active checkpoint that shouldn't be overwritten
    // This prevents TOCTOU race conditions where multiple processes try to index
    // the same workspace simultaneously.
    let existing: Option<(String, i64)> = conn
        .query_row(
            "SELECT phase, updated_at FROM checkpoint WHERE workspace = ?",
            params![workspace],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    if let Some((phase, updated_at)) = existing {
        let is_active = phase == "scanning" || phase == "indexing";
        let is_stale = now - updated_at > STALE_THRESHOLD_SECS;

        if is_active && !is_stale {
            return Err(RetrievalErr::IndexingInProgress {
                workspace: workspace.to_string(),
                phase,
                started_secs_ago: now - updated_at,
            });
        }
    }

    // Safe to create/replace checkpoint - either no existing, completed/failed, or stale
    conn.execute(
        "INSERT OR REPLACE INTO checkpoint (id, workspace, phase, total_files, processed_files, last_file, started_at, updated_at)
         VALUES (1, ?, 'scanning', ?, 0, NULL, ?, ?)",
        params![workspace, total_files, now, now],
    )
    .map_err(RetrievalErr::from)?;

    Ok(())
}

fn load_checkpoint(conn: &Connection, workspace: &str) -> Result<Option<CheckpointState>> {
    conn.query_row(
        "SELECT workspace, phase, total_files, processed_files, last_file, started_at, updated_at
         FROM checkpoint
         WHERE workspace = ?",
        params![workspace],
        |row| {
            Ok(CheckpointState {
                workspace: row.get(0)?,
                phase: IndexPhase::from_str(&row.get::<_, String>(1)?),
                total_files: row.get(2)?,
                processed_files: row.get(3)?,
                last_file: row.get(4)?,
                started_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        },
    )
    .optional()
}

fn current_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Builder for resuming from checkpoint.
pub struct ResumeBuilder {
    checkpoint: Arc<Checkpoint>,
    workspace: String,
}

impl ResumeBuilder {
    /// Create a new resume builder.
    pub fn new(checkpoint: Arc<Checkpoint>, workspace: &str) -> Self {
        Self {
            checkpoint,
            workspace: workspace.to_string(),
        }
    }

    /// Check if resume is possible.
    pub async fn can_resume(&self) -> Result<bool> {
        self.checkpoint.has_resumable(&self.workspace).await
    }

    /// Get resume state.
    pub async fn get_state(&self) -> Result<Option<CheckpointState>> {
        self.checkpoint.load(&self.workspace).await
    }

    /// Get files to skip (already processed).
    pub async fn get_skip_until(&self) -> Result<Option<String>> {
        self.checkpoint.get_resume_point(&self.workspace).await
    }
}

#[cfg(test)]
#[path = "checkpoint.test.rs"]
mod tests;
