use super::*;
use crate::indexing::FileIndexLocks;
use crate::indexing::WatchEventKind;
use crate::indexing::new_watch_event_queue;
use std::sync::atomic::AtomicI32;

/// Test processor that counts processed events.
#[derive(Debug)]
struct CountingProcessor {
    count: AtomicI32,
}

impl CountingProcessor {
    fn new() -> Self {
        Self {
            count: AtomicI32::new(0),
        }
    }

    fn count(&self) -> i32 {
        self.count.load(Ordering::Acquire)
    }
}

#[async_trait]
impl EventProcessor for CountingProcessor {
    type EventData = WatchEventKind;

    async fn process(&self, _path: &Path, _event: &TrackedEvent<Self::EventData>) -> Result<()> {
        self.count.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn name(&self) -> &str {
        "counting-processor"
    }
}

/// Test processor that fails on specific paths.
#[derive(Debug)]
struct FailingProcessor {
    fail_pattern: String,
}

impl FailingProcessor {
    fn new(fail_pattern: &str) -> Self {
        Self {
            fail_pattern: fail_pattern.to_string(),
        }
    }
}

#[async_trait]
impl EventProcessor for FailingProcessor {
    type EventData = WatchEventKind;

    async fn process(&self, path: &Path, _event: &TrackedEvent<Self::EventData>) -> Result<()> {
        if path.to_string_lossy().contains(&self.fail_pattern) {
            Err(crate::error::RetrievalErr::SearchFailed {
                query: "test".to_string(),
                cause: "Simulated failure".to_string(),
            })
        } else {
            Ok(())
        }
    }

    fn name(&self) -> &str {
        "failing-processor"
    }
}

#[allow(clippy::type_complexity)]
fn create_test_pool<P: EventProcessor<EventData = WatchEventKind> + 'static>(
    processor: Arc<P>,
) -> (
    Arc<WorkerPool<PathBuf, WatchEventKind, P>>,
    Arc<crate::indexing::WatchEventQueue>,
    Arc<LagTracker>,
    Arc<BatchTracker>,
) {
    let queue = Arc::new(new_watch_event_queue(64));
    let file_locks = Arc::new(FileIndexLocks::new());
    let batch_tracker = Arc::new(BatchTracker::new());
    let lag_tracker = Arc::new(LagTracker::new());
    let cancel = CancellationToken::new();

    let config = WorkerPoolConfig {
        worker_count: 2,
        requeue_delay_ms: 1,
    };

    let pool = Arc::new(WorkerPool::new(
        Arc::clone(&queue),
        processor,
        file_locks,
        Arc::clone(&batch_tracker),
        Arc::clone(&lag_tracker),
        cancel,
        config,
    ));

    (pool, queue, lag_tracker, batch_tracker)
}

#[tokio::test]
async fn test_worker_pool_basic() {
    let processor = Arc::new(CountingProcessor::new());
    let (pool, queue, lag_tracker, _) = create_test_pool(Arc::clone(&processor));

    // Start the pool
    pool.start();

    // Push some events with seq numbers
    for i in 0..5 {
        let seq = lag_tracker.assign_seq();
        let event = TrackedEvent::new(WatchEventKind::Changed, None, seq, format!("trace-{i}"));
        queue
            .push(PathBuf::from(format!("file{i}.rs")), event)
            .await;
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(processor.count(), 5);
    assert_eq!(lag_tracker.current_lag(), 0);

    pool.stop();
}

#[tokio::test]
async fn test_worker_pool_with_batch() {
    let processor = Arc::new(CountingProcessor::new());
    let (pool, queue, lag_tracker, batch_tracker) = create_test_pool(Arc::clone(&processor));

    pool.start();

    // Create a batch
    let batch_id = crate::indexing::BatchId::new();
    let rx = batch_tracker.start_batch(batch_id.clone(), 3).await;

    // Push events with batch_id
    for i in 0..3 {
        let seq = lag_tracker.assign_seq();
        let event = TrackedEvent::new(
            WatchEventKind::Changed,
            Some(batch_id.clone()),
            seq,
            format!("batch-trace-{i}"),
        );
        queue
            .push(PathBuf::from(format!("batch{i}.rs")), event)
            .await;
    }

    // Wait for batch completion
    let result = tokio::time::timeout(Duration::from_secs(1), rx).await;
    assert!(result.is_ok());

    let batch_result = result.unwrap().unwrap();
    assert_eq!(batch_result.completed, 3);
    assert_eq!(batch_result.failed, 0);

    pool.stop();
}

#[tokio::test]
async fn test_worker_pool_with_failures() {
    let processor = Arc::new(FailingProcessor::new("fail"));
    let (pool, queue, lag_tracker, batch_tracker) = create_test_pool(Arc::clone(&processor));

    pool.start();

    let batch_id = crate::indexing::BatchId::new();
    let rx = batch_tracker.start_batch(batch_id.clone(), 3).await;

    // One will fail, two will succeed
    let paths = ["ok1.rs", "fail.rs", "ok2.rs"];
    for (i, path) in paths.iter().enumerate() {
        let seq = lag_tracker.assign_seq();
        let event = TrackedEvent::new(
            WatchEventKind::Changed,
            Some(batch_id.clone()),
            seq,
            format!("trace-{i}"),
        );
        queue.push(PathBuf::from(path), event).await;
    }

    // Wait for batch completion
    let result = tokio::time::timeout(Duration::from_secs(1), rx).await;
    assert!(result.is_ok());

    let batch_result = result.unwrap().unwrap();
    assert_eq!(batch_result.completed, 2);
    assert_eq!(batch_result.failed, 1);

    // Lag should still be 0 (failed events don't block)
    assert_eq!(lag_tracker.current_lag(), 0);

    // Check failed count in lag tracker
    let info = lag_tracker.lag_info().await;
    assert_eq!(info.failed_count, 1);

    pool.stop();
}

#[tokio::test]
async fn test_worker_pool_stop() {
    let processor = Arc::new(CountingProcessor::new());
    let (pool, _, _, _) = create_test_pool(Arc::clone(&processor));

    pool.start();

    // Wait for workers to start
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(pool.active_workers() > 0);

    // Stop the pool
    pool.stop();

    // Wait for workers to stop
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(pool.is_stopped());
}

#[tokio::test]
async fn test_worker_pool_lag_tracking() {
    let processor = Arc::new(CountingProcessor::new());
    let (pool, queue, lag_tracker, _) = create_test_pool(Arc::clone(&processor));

    // Assign seq numbers before starting pool
    let seqs: Vec<i64> = (0..5).map(|_| lag_tracker.assign_seq()).collect();

    // Push events
    for (i, seq) in seqs.iter().enumerate() {
        let event = TrackedEvent::new(WatchEventKind::Changed, None, *seq, format!("trace-{i}"));
        queue
            .push(PathBuf::from(format!("file{i}.rs")), event)
            .await;
    }

    // Lag should be 5 before processing
    assert_eq!(lag_tracker.current_lag(), 5);

    pool.start();

    // Wait for processing
    let result = lag_tracker.wait_for_zero_lag(Duration::from_secs(1)).await;
    assert!(result.is_ok());

    assert_eq!(lag_tracker.current_lag(), 0);

    pool.stop();
}
