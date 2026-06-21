//! Index coordinator for managing indexing operations.
//!
//! Coordinates session startup detection, timer-based periodic checks,
//! and file watcher events with proper concurrency control.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use super::event_queue::SharedEventQueue;
use super::event_queue::new_watch_event_queue;
use super::file_locks::FileIndexLocks;
use super::file_locks::SharedFileLocks;
use super::manager::IndexManager;
use super::manager::IndexStats;
use super::watcher::WatchEventKind;
use crate::config::RetrievalConfig;
use crate::error::Result;
use crate::storage::SqliteStore;

/// Index state representing the current status of the index.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum IndexState {
    /// Index has not been initialized yet.
    #[default]
    Uninitialized,
    /// Index is currently being built.
    Building {
        /// Progress percentage (0.0 - 1.0).
        progress: f32,
        /// Unix timestamp when building started.
        started_at: i64,
    },
    /// Index is ready for search.
    Ready {
        /// Index statistics.
        stats: IndexStats,
        /// Unix timestamp when indexing completed.
        indexed_at: i64,
    },
    /// Index is stale and needs incremental update.
    Stale {
        /// Current index statistics.
        stats: IndexStats,
        /// Reason for staleness.
        reason: StaleReason,
    },
    /// Index build failed.
    Failed {
        /// Error message.
        error: String,
        /// Unix timestamp when failure occurred.
        failed_at: i64,
    },
}

/// Reason why the index is considered stale.
#[derive(Debug, Clone, PartialEq)]
pub enum StaleReason {
    /// Files have newer mtime than indexed mtime.
    MtimeChanged {
        /// Number of files with newer mtime.
        newer_files: i32,
    },
    /// Periodic timer triggered a check.
    TimerTriggered,
    /// File watcher detected changes.
    WatcherTriggered {
        /// Paths of changed files.
        changed_files: Vec<String>,
    },
}

/// Result of freshness check.
#[derive(Debug, Clone)]
pub enum FreshnessResult {
    /// Index is fresh, no updates needed.
    Fresh,
    /// Index is stale with detected changes.
    Stale {
        /// List of detected file changes.
        changes: Vec<CoordinatorFileChange>,
    },
    /// Check was superseded by another operation.
    Superseded,
}

/// File change detected by the coordinator.
///
/// Simplified to a single variant - the processor checks file existence
/// to determine the actual action (update if exists, delete if not).
#[derive(Debug, Clone)]
pub struct CoordinatorFileChange(pub PathBuf);

impl CoordinatorFileChange {
    /// Create a new file change event.
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    /// Get the path of the changed file.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Convert to WatchEventKind.
    pub fn to_event_kind(&self) -> WatchEventKind {
        WatchEventKind::Changed
    }
}

/// Source of index trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerSource {
    /// Triggered at session startup.
    SessionStart,
    /// Triggered by periodic timer.
    Timer,
    /// Triggered by file watcher.
    Watcher,
    /// Triggered manually.
    Manual,
}

/// Index coordinator for managing all indexing operations.
///
/// Handles:
/// - Session startup index detection
/// - Timer-based periodic mtime checks
/// - File watcher event processing
/// - Concurrent access control via epoch optimistic locking
pub struct IndexCoordinator {
    /// Current index state.
    state: RwLock<IndexState>,
    /// Epoch for optimistic locking (incremented on each index operation).
    epoch: AtomicI64,
    /// Event queue for watch events.
    event_queue: SharedEventQueue,
    /// File-level locks for concurrent processing.
    file_locks: SharedFileLocks,
    /// Configuration.
    config: RetrievalConfig,
    /// Workspace identifier.
    workspace: String,
    /// Working directory root.
    workdir: PathBuf,
    /// SQLite store for catalog queries.
    db: Arc<SqliteStore>,
    /// Cancellation token for stopping workers and timer.
    cancel: CancellationToken,
    /// Number of active workers.
    worker_count: AtomicI64,
}

impl IndexCoordinator {
    /// Create a new index coordinator.
    ///
    /// # Arguments
    /// * `config` - Retrieval configuration
    /// * `workspace` - Workspace identifier
    /// * `workdir` - Working directory root
    /// * `db` - SQLite store for catalog queries
    pub fn new(
        config: RetrievalConfig,
        workspace: String,
        workdir: PathBuf,
        db: Arc<SqliteStore>,
    ) -> Self {
        let event_queue = Arc::new(new_watch_event_queue(256));
        let file_locks = Arc::new(FileIndexLocks::new());

        Self {
            state: RwLock::new(IndexState::Uninitialized),
            epoch: AtomicI64::new(0),
            event_queue,
            file_locks,
            config,
            workspace,
            workdir,
            db,
            cancel: CancellationToken::new(),
            worker_count: AtomicI64::new(0),
        }
    }

    /// Get the current epoch value.
    pub fn epoch(&self) -> i64 {
        self.epoch.load(Ordering::Acquire)
    }

    /// Get the current index state.
    pub async fn state(&self) -> IndexState {
        self.state.read().await.clone()
    }

    /// Check if the index is ready for search.
    pub async fn is_ready(&self) -> bool {
        matches!(
            *self.state.read().await,
            IndexState::Ready { .. } | IndexState::Stale { .. }
        )
    }

    /// Get the event queue (for file watcher integration).
    pub fn event_queue(&self) -> SharedEventQueue {
        self.event_queue.clone()
    }

    /// Get the file locks (for external coordination).
    pub fn file_locks(&self) -> SharedFileLocks {
        self.file_locks.clone()
    }

    /// Get the cancellation token.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Trigger index check from the specified source.
    ///
    /// This is the main entry point for triggering index operations.
    /// Uses epoch optimistic locking to handle concurrent triggers.
    pub async fn try_trigger_index(&self, source: TriggerSource) -> Result<()> {
        tracing::debug!(source = ?source, "Index trigger requested");

        // Check current state
        let current_state = self.state.read().await.clone();

        match current_state {
            IndexState::Building { .. } => {
                // Already building, skip
                tracing::debug!("Index already building, skipping trigger");
                return Ok(());
            }
            IndexState::Uninitialized | IndexState::Failed { .. } => {
                // Need full rebuild
                self.trigger_full_rebuild(source).await?;
            }
            IndexState::Ready { .. } | IndexState::Stale { .. } => {
                // Check freshness and trigger incremental if needed
                self.trigger_freshness_check(source).await?;
            }
        }

        Ok(())
    }

    /// Trigger a full index rebuild.
    async fn trigger_full_rebuild(&self, source: TriggerSource) -> Result<()> {
        tracing::info!(source = ?source, workspace = %self.workspace, "Triggering full index rebuild");

        // Update state to building
        {
            let mut state = self.state.write().await;
            *state = IndexState::Building {
                progress: 0.0,
                started_at: chrono::Utc::now().timestamp(),
            };
        }

        // Increment epoch
        self.epoch.fetch_add(1, Ordering::AcqRel);

        // Note: Actual indexing is done by IndexManager in RetrievalService
        // This coordinator just manages state and triggers
        // The service should call mark_building_complete() when done

        Ok(())
    }

    /// Trigger a freshness check and incremental update if needed.
    async fn trigger_freshness_check(&self, source: TriggerSource) -> Result<()> {
        // Record current epoch for optimistic locking
        let epoch = self.epoch.load(Ordering::Acquire);

        // Perform freshness check (lock-free)
        let result = self.check_freshness().await?;

        // Verify epoch hasn't changed
        if self.epoch.load(Ordering::Acquire) != epoch {
            tracing::debug!("Freshness check superseded by another operation");
            return Ok(());
        }

        match result {
            FreshnessResult::Fresh => {
                tracing::debug!("Index is fresh, no updates needed");
            }
            FreshnessResult::Stale { changes } => {
                tracing::info!(
                    source = ?source,
                    changes = changes.len(),
                    "Index is stale, queueing updates"
                );

                // Push changes to event queue
                for change in changes {
                    let kind = change.to_event_kind();
                    let path = change.path().to_path_buf();
                    self.event_queue.push_simple(path, kind).await;
                }

                // Update state to stale
                let stats = self.get_stats().await?;
                let reason = match source {
                    TriggerSource::Timer => StaleReason::TimerTriggered,
                    TriggerSource::Watcher => StaleReason::WatcherTriggered {
                        changed_files: Vec::new(),
                    },
                    _ => StaleReason::MtimeChanged { newer_files: 0 },
                };

                {
                    let mut state = self.state.write().await;
                    *state = IndexState::Stale { stats, reason };
                }
            }
            FreshnessResult::Superseded => {
                tracing::debug!("Freshness check superseded");
            }
        }

        Ok(())
    }

    /// Check index freshness by comparing mtimes (lock-free).
    ///
    /// This method collects file changes without holding any locks.
    /// Uses epoch optimistic locking for validation.
    pub async fn check_freshness(&self) -> Result<FreshnessResult> {
        use super::change_detector::CatalogEntry;
        use super::walker::FileWalker;

        tracing::trace!(
            workspace = %self.workspace,
            workdir = %self.workdir.display(),
            "Checking index freshness"
        );

        // Record epoch at start
        let epoch = self.epoch.load(Ordering::Acquire);

        // Get indexed files from catalog
        let workspace = self.workspace.clone();
        let indexed_files: Vec<CatalogEntry> = self
            .db
            .query(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT filepath, content_hash, mtime, indexed_at, chunks_count, chunks_failed
                     FROM catalog WHERE workspace = ?",
                )?;
                let rows = stmt.query_map([&workspace], |row| {
                    Ok(CatalogEntry {
                        filepath: row.get(0)?,
                        content_hash: row.get(1)?,
                        mtime: row.get(2)?,
                        indexed_at: row.get(3)?,
                        chunks_count: row.get(4)?,
                        chunks_failed: row.get(5)?,
                    })
                })?;
                let mut results = Vec::new();
                for row in rows {
                    results.push(row?);
                }
                Ok(results)
            })
            .await?;

        // Check if epoch changed during catalog read
        if self.epoch.load(Ordering::Acquire) != epoch {
            return Ok(FreshnessResult::Superseded);
        }

        // Build indexed files map
        let indexed_map: std::collections::HashMap<String, i64> = indexed_files
            .into_iter()
            .map(|e| (e.filepath, e.mtime))
            .collect();

        // Walk current files
        let walker = FileWalker::with_filter(
            &self.workdir,
            self.config.indexing.max_file_size_mb,
            &self.config.indexing.include_dirs,
            &self.config.indexing.exclude_dirs,
            &self.config.indexing.include_extensions,
            &self.config.indexing.exclude_extensions,
        );
        let current_files = walker.walk(&self.workdir)?;

        // Check if epoch changed during file walk
        if self.epoch.load(Ordering::Acquire) != epoch {
            return Ok(FreshnessResult::Superseded);
        }

        // Collect changes
        let mut changes = Vec::new();
        let current_paths: std::collections::HashSet<String> = current_files
            .iter()
            .filter_map(|p| {
                p.strip_prefix(&self.workdir)
                    .ok()
                    .map(|r| r.to_string_lossy().to_string())
            })
            .collect();

        // Check for modified and added files
        for path in &current_files {
            let rel_path = path
                .strip_prefix(&self.workdir)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let current_mtime = super::change_detector::get_mtime(path).unwrap_or(0);

            match indexed_map.get(&rel_path) {
                Some(&indexed_mtime) => {
                    if current_mtime > indexed_mtime {
                        // File modified - processor will check existence
                        changes.push(CoordinatorFileChange::new(path.clone()));
                    }
                }
                None => {
                    // File added - processor will check existence
                    changes.push(CoordinatorFileChange::new(path.clone()));
                }
            }
        }

        // Check for deleted files - processor will check existence
        for rel_path in indexed_map.keys() {
            if !current_paths.contains(rel_path) {
                let full_path = self.workdir.join(rel_path);
                changes.push(CoordinatorFileChange::new(full_path));
            }
        }

        // Final epoch check
        if self.epoch.load(Ordering::Acquire) != epoch {
            return Ok(FreshnessResult::Superseded);
        }

        if changes.is_empty() {
            Ok(FreshnessResult::Fresh)
        } else {
            Ok(FreshnessResult::Stale { changes })
        }
    }

    /// Get index statistics from the catalog.
    async fn get_stats(&self) -> Result<IndexStats> {
        let workspace = self.workspace.clone();

        let (file_count, chunk_count, last_indexed) = self
            .db
            .query(move |conn| {
                let file_count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM catalog WHERE workspace = ?",
                        [&workspace],
                        |row| row.get(0),
                    )
                    .unwrap_or(0);

                let chunk_count: i64 = conn
                    .query_row(
                        "SELECT COALESCE(SUM(chunks_count), 0) FROM catalog WHERE workspace = ?",
                        [&workspace],
                        |row| row.get(0),
                    )
                    .unwrap_or(0);

                let last_indexed: Option<i64> = conn
                    .query_row(
                        "SELECT MAX(indexed_at) FROM catalog WHERE workspace = ?",
                        [&workspace],
                        |row| row.get(0),
                    )
                    .ok()
                    .flatten();

                Ok((file_count, chunk_count, last_indexed))
            })
            .await?;

        Ok(IndexStats {
            file_count,
            chunk_count,
            last_indexed,
        })
    }

    /// Mark index building as started.
    pub async fn mark_building_started(&self) {
        let mut state = self.state.write().await;
        *state = IndexState::Building {
            progress: 0.0,
            started_at: chrono::Utc::now().timestamp(),
        };
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }

    /// Update building progress.
    pub async fn update_building_progress(&self, progress: f32) {
        let mut state = self.state.write().await;
        if let IndexState::Building { started_at, .. } = *state {
            *state = IndexState::Building {
                progress,
                started_at,
            };
        }
    }

    /// Mark index building as complete.
    pub async fn mark_building_complete(&self, stats: IndexStats) {
        let mut state = self.state.write().await;
        *state = IndexState::Ready {
            stats,
            indexed_at: chrono::Utc::now().timestamp(),
        };
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }

    /// Mark index building as failed.
    pub async fn mark_building_failed(&self, error: String) {
        let mut state = self.state.write().await;
        *state = IndexState::Failed {
            error,
            failed_at: chrono::Utc::now().timestamp(),
        };
    }

    /// Start worker threads for processing events.
    ///
    /// # Arguments
    /// * `count` - Number of worker threads to start
    /// * `manager` - Index manager for processing individual files
    pub fn start_workers(self: &Arc<Self>, count: i32, manager: Arc<IndexManager>) {
        for id in 0..count {
            let coord = Arc::clone(self);
            let mgr = Arc::clone(&manager);
            tokio::spawn(async move {
                coord.worker_loop(id, mgr).await;
            });
        }
        self.worker_count.store(count as i64, Ordering::Release);
    }

    /// Worker loop for processing events from the queue.
    async fn worker_loop(self: Arc<Self>, id: i32, _manager: Arc<IndexManager>) {
        tracing::debug!(worker_id = id, "Worker started");

        let mut rx = self.event_queue.subscribe();

        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    tracing::debug!(worker_id = id, "Worker cancelled");
                    break;
                }
                _ = rx.recv() => {
                    self.process_pending_events().await;
                }
            }
        }

        tracing::debug!(worker_id = id, "Worker stopped");
    }

    /// Process pending events from the queue.
    async fn process_pending_events(&self) {
        while let Some((path, event)) = self.event_queue.pop().await {
            // Try to acquire file lock
            if let Some(_guard) = self.file_locks.try_lock(&path).await {
                // Process the file event (extract data from TrackedEvent)
                self.process_file_event(&path, event.data).await;
                // Clean up lock
                self.file_locks.cleanup(&path).await;
            } else {
                // Lock contention, requeue the event
                self.event_queue.requeue(path, event).await;
                // Brief sleep to avoid busy loop
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }

    /// Process a single file event.
    async fn process_file_event(&self, path: &Path, _kind: WatchEventKind) {
        tracing::debug!(path = %path.display(), "Processing file event");

        // Increment epoch
        self.epoch.fetch_add(1, Ordering::AcqRel);

        // Check file existence to determine action
        // This is more robust than trusting event type (handles race conditions)
        if path.exists() {
            // TODO: Index single file via IndexManager
            tracing::debug!(path = %path.display(), "Would index file");
        } else {
            // TODO: Remove file from index via IndexManager
            tracing::debug!(path = %path.display(), "Would remove file from index");
        }
    }

    /// Start periodic timer for mtime checks.
    ///
    /// # Arguments
    /// * `interval` - Check interval duration
    pub fn start_timer(self: &Arc<Self>, interval: Duration) {
        if interval.is_zero() {
            tracing::debug!("Timer disabled (interval is zero)");
            return;
        }

        let coord = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(interval);

            loop {
                tokio::select! {
                    _ = coord.cancel.cancelled() => {
                        tracing::debug!("Timer cancelled");
                        break;
                    }
                    _ = interval_timer.tick() => {
                        if let Err(e) = coord.try_trigger_index(TriggerSource::Timer).await {
                            tracing::warn!(error = %e, "Timer-triggered index check failed");
                        }
                    }
                }
            }
        });

        tracing::debug!(interval_secs = interval.as_secs(), "Timer started");
    }

    /// Stop all workers and timers.
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// Check if coordinator is stopped.
    pub fn is_stopped(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

impl std::fmt::Debug for IndexCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexCoordinator")
            .field("workspace", &self.workspace)
            .field("workdir", &self.workdir)
            .field("epoch", &self.epoch.load(Ordering::Relaxed))
            .finish()
    }
}

/// Shared index coordinator wrapped in Arc for use across threads.
pub type SharedCoordinator = Arc<IndexCoordinator>;

#[cfg(test)]
#[path = "coordinator.test.rs"]
mod tests;
