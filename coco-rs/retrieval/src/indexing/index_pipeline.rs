//! Index pipeline for search functionality.
//!
//! Encapsulates the indexing workflow including:
//! - File chunking and embedding generation
//! - BM25 full-text indexing
//! - Vector storage updates
//!
//! ## Architecture
//!
//! ```text
//!    TriggerSource (SessionStart/Timer/Watcher)
//!          │
//!          ▼
//!    IndexEventQueue (dedup by path)
//!          │
//!          ▼
//!    IndexWorkerPool
//!          │
//!          ├─► IndexEventProcessor
//!          │     ├─ Read file content
//!          │     ├─ Parse with tree-sitter
//!          │     ├─ Split into chunks
//!          │     ├─ Generate embeddings
//!          │     └─ Store in SQLite (metadata + vectors)
//!          │
//!          ├─► BatchTracker (SessionStart completion)
//!          └─► LagTracker (watermark-based lag)
//!                    │
//!                    ▼
//!              Readiness check
//! ```

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use super::BatchId;
use super::BatchResult;
use super::BatchTracker;
use super::EventProcessor;
use super::FileIndexLocks;
use super::IndexStats;
use super::LagInfo;
use super::LagTracker;
use super::TrackedEvent;
use super::WatchEventKind;
use super::WorkerPool;
use super::WorkerPoolConfig;
use super::new_watch_event_queue;
use super::pipeline_common::PipelineReadiness;
use super::pipeline_common::PipelineState;
use super::pipeline_common::compute_readiness;
use super::pipeline_common::now_timestamp;

// Re-export StrictModeConfig for backward compatibility
pub use super::pipeline_common::StrictModeConfig;
use crate::config::RetrievalConfig;
use crate::error::Result;
use crate::storage::SqliteStore;

/// Type alias for index pipeline state using common generic type.
pub type IndexPipelineState = PipelineState<IndexStats>;

/// Type alias for index pipeline readiness using common generic type.
pub type Readiness = PipelineReadiness<IndexStats>;

/// Index event processor that handles file indexing.
#[allow(dead_code)] // Fields will be used when indexing logic is implemented
pub struct IndexEventProcessor {
    /// SQLite store for metadata and BM25.
    db: Arc<SqliteStore>,
    /// Configuration.
    config: RetrievalConfig,
    /// Workspace root directory.
    workdir: PathBuf,
}

impl std::fmt::Debug for IndexEventProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexEventProcessor")
            .field("workdir", &self.workdir)
            .finish()
    }
}

impl IndexEventProcessor {
    /// Create a new index event processor.
    pub fn new(db: Arc<SqliteStore>, config: RetrievalConfig, workdir: PathBuf) -> Self {
        Self {
            db,
            config,
            workdir,
        }
    }
}

#[async_trait]
impl EventProcessor for IndexEventProcessor {
    type EventData = WatchEventKind;

    async fn process(&self, path: &Path, event: &TrackedEvent<Self::EventData>) -> Result<()> {
        let trace_id = &event.trace_id;

        tracing::debug!(
            trace_id = %trace_id,
            path = %path.display(),
            kind = ?event.data,
            "IndexEventProcessor: processing file"
        );

        // Check file existence to determine action
        // This is more robust than trusting event type (handles race conditions)
        if path.exists() {
            // File exists - index/update it
            // TODO: Implement full indexing logic
            // 1. Read file content
            // 2. Parse with tree-sitter
            // 3. Split into chunks
            // 4. Generate embeddings (if vector search enabled)
            // 5. Store in SQLite (metadata, BM25, vectors)
            tracing::debug!(
                trace_id = %trace_id,
                path = %path.display(),
                "Would index file (not implemented yet)"
            );
        } else {
            // File doesn't exist - remove from index
            // TODO: Implement removal logic
            // 1. Delete from vector store
            // 2. Delete from BM25 index
            // 3. Remove from catalog
            tracing::debug!(
                trace_id = %trace_id,
                path = %path.display(),
                "Would remove file from index (not implemented yet)"
            );
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "index-processor"
    }
}

/// Type alias for the index worker pool.
pub type IndexWorkerPool = WorkerPool<PathBuf, WatchEventKind, IndexEventProcessor>;

/// Index pipeline for search functionality.
pub struct IndexPipeline {
    /// Current state of the pipeline.
    state: RwLock<IndexPipelineState>,
    /// Event queue for file changes.
    event_queue: Arc<super::WatchEventQueue>,
    /// File-level locks.
    file_locks: Arc<FileIndexLocks>,
    /// Batch tracker for SessionStart completion.
    batch_tracker: Arc<BatchTracker>,
    /// Lag tracker for watermark mechanism.
    lag_tracker: Arc<LagTracker>,
    /// Cancellation token.
    cancel: CancellationToken,
    /// Event processor.
    processor: Arc<IndexEventProcessor>,
    /// Worker pool (initialized lazily).
    worker_pool: RwLock<Option<Arc<IndexWorkerPool>>>,
    /// Whether initial build has completed.
    init_complete: AtomicBool,
    /// Strict mode configuration.
    strict_config: StrictModeConfig,
    /// Worker pool configuration.
    worker_config: WorkerPoolConfig,
}

impl IndexPipeline {
    /// Create a new index pipeline.
    pub fn new(
        db: Arc<SqliteStore>,
        config: RetrievalConfig,
        workdir: PathBuf,
        strict_config: StrictModeConfig,
    ) -> Self {
        let event_queue = Arc::new(new_watch_event_queue(256));
        let file_locks = Arc::new(FileIndexLocks::new());
        let batch_tracker = Arc::new(BatchTracker::new());
        let lag_tracker = Arc::new(LagTracker::new());
        let cancel = CancellationToken::new();

        let processor = Arc::new(IndexEventProcessor::new(db, config.clone(), workdir));

        let worker_config = WorkerPoolConfig {
            worker_count: config.indexing.worker_count,
            requeue_delay_ms: 10,
        };

        Self {
            state: RwLock::new(PipelineState::Uninitialized),
            event_queue,
            file_locks,
            batch_tracker,
            lag_tracker,
            cancel,
            processor,
            worker_pool: RwLock::new(None),
            init_complete: AtomicBool::new(false),
            strict_config,
            worker_config,
        }
    }

    /// Start the worker pool.
    pub async fn start_workers(&self) {
        let mut pool_guard = self.worker_pool.write().await;
        if pool_guard.is_none() {
            let pool = Arc::new(WorkerPool::new(
                Arc::clone(&self.event_queue),
                Arc::clone(&self.processor),
                Arc::clone(&self.file_locks),
                Arc::clone(&self.batch_tracker),
                Arc::clone(&self.lag_tracker),
                self.cancel.clone(),
                self.worker_config.clone(),
            ));
            pool.start();
            *pool_guard = Some(pool);

            tracing::info!("Index pipeline workers started");
        }
    }

    /// Stop the worker pool.
    pub async fn stop(&self) {
        self.cancel.cancel();
        tracing::info!("Index pipeline stopped");
    }

    /// Check if the pipeline is stopped.
    pub fn is_stopped(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Get the current state.
    pub async fn state(&self) -> IndexPipelineState {
        self.state.read().await.clone()
    }

    /// Mark the pipeline as building.
    pub async fn mark_building(&self, batch_id: BatchId) {
        *self.state.write().await = PipelineState::Building {
            batch_id,
            progress: 0.0,
            started_at: now_timestamp(),
        };
    }

    /// Update building progress.
    pub async fn update_progress(&self, progress: f32) {
        let mut state = self.state.write().await;
        if let PipelineState::Building {
            batch_id,
            started_at,
            ..
        } = &*state
        {
            *state = PipelineState::Building {
                batch_id: batch_id.clone(),
                progress,
                started_at: *started_at,
            };
        }
    }

    /// Mark the pipeline as ready.
    ///
    /// Also triggers cleanup of file locks to prevent memory leaks from
    /// any locks that might have been missed during per-file cleanup.
    pub async fn mark_ready(&self, stats: IndexStats) {
        *self.state.write().await = PipelineState::Ready {
            stats,
            completed_at: now_timestamp(),
        };
        self.init_complete.store(true, Ordering::Release);

        // Cleanup any remaining file locks to prevent memory leaks
        self.file_locks.cleanup_all().await;
        tracing::debug!("Cleaned up file locks after index pipeline completion");
    }

    /// Mark the pipeline as failed.
    pub async fn mark_failed(&self, error: String) {
        *self.state.write().await = PipelineState::Failed {
            error,
            failed_at: now_timestamp(),
        };
    }

    /// Get the event queue for pushing events.
    pub fn event_queue(&self) -> Arc<super::WatchEventQueue> {
        Arc::clone(&self.event_queue)
    }

    /// Get the batch tracker.
    pub fn batch_tracker(&self) -> Arc<BatchTracker> {
        Arc::clone(&self.batch_tracker)
    }

    /// Get the lag tracker.
    pub fn lag_tracker(&self) -> Arc<LagTracker> {
        Arc::clone(&self.lag_tracker)
    }

    /// Assign a sequence number for a new event.
    pub fn assign_seq(&self) -> i64 {
        self.lag_tracker.assign_seq()
    }

    /// Start a new batch for SessionStart.
    pub async fn start_batch(
        &self,
        batch_id: BatchId,
        total: i64,
    ) -> tokio::sync::oneshot::Receiver<BatchResult> {
        self.batch_tracker.start_batch(batch_id, total).await
    }

    /// Push an event to the queue.
    pub async fn push_event(&self, path: PathBuf, event: TrackedEvent<WatchEventKind>) {
        self.event_queue.push(path, event).await;
    }

    /// Push a simple event without tracking.
    pub async fn push_simple(&self, path: PathBuf, kind: WatchEventKind) {
        self.event_queue.push_simple(path, kind).await;
    }

    /// Get current lag.
    pub fn current_lag(&self) -> i64 {
        self.lag_tracker.current_lag()
    }

    /// Get detailed lag info.
    pub async fn lag_info(&self) -> LagInfo {
        self.lag_tracker.lag_info().await
    }

    /// Check if initial build is complete.
    pub fn is_init_complete(&self) -> bool {
        self.init_complete.load(Ordering::Acquire)
    }

    /// Get the readiness status.
    pub async fn readiness(&self) -> Readiness {
        let state = self.state.read().await.clone();
        let lag_info = self.lag_tracker.lag_info().await;
        compute_readiness(
            &state,
            lag_info,
            self.is_init_complete(),
            &self.strict_config,
        )
    }

    /// Check if ready for search (quick check).
    pub async fn is_ready(&self) -> bool {
        matches!(self.readiness().await, Readiness::Ready { .. })
    }
}

impl std::fmt::Debug for IndexPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexPipeline")
            .field("is_stopped", &self.is_stopped())
            .field("init_complete", &self.is_init_complete())
            .field("current_lag", &self.current_lag())
            .finish()
    }
}

/// Shared index pipeline.
pub type SharedIndexPipeline = Arc<IndexPipeline>;

#[cfg(test)]
#[path = "index_pipeline.test.rs"]
mod tests;
