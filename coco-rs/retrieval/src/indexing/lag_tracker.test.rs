use super::*;

#[tokio::test]
async fn test_sequential_completion() {
    let tracker = LagTracker::new();

    // Assign 3 sequences
    let seq1 = tracker.assign_seq();
    let seq2 = tracker.assign_seq();
    let seq3 = tracker.assign_seq();

    assert_eq!(seq1, 1);
    assert_eq!(seq2, 2);
    assert_eq!(seq3, 3);
    assert_eq!(tracker.current_lag(), 3);

    // Start all
    tracker.start_event(seq1).await;
    tracker.start_event(seq2).await;
    tracker.start_event(seq3).await;

    // Complete in order
    tracker.complete_event(seq1).await;
    assert_eq!(tracker.watermark(), 1);
    assert_eq!(tracker.current_lag(), 2);

    tracker.complete_event(seq2).await;
    assert_eq!(tracker.watermark(), 2);
    assert_eq!(tracker.current_lag(), 1);

    tracker.complete_event(seq3).await;
    assert_eq!(tracker.watermark(), 3);
    assert_eq!(tracker.current_lag(), 0);
}

#[tokio::test]
async fn test_out_of_order_completion() {
    let tracker = LagTracker::new();

    // Assign 5 sequences
    let seq1 = tracker.assign_seq();
    let seq2 = tracker.assign_seq();
    let seq3 = tracker.assign_seq();
    let seq4 = tracker.assign_seq();
    let seq5 = tracker.assign_seq();

    // Start all
    for seq in [seq1, seq2, seq3, seq4, seq5] {
        tracker.start_event(seq).await;
    }

    assert_eq!(tracker.current_lag(), 5);

    // Complete out of order: 3, 1, 5, 2, 4
    tracker.complete_event(seq3).await;
    assert_eq!(tracker.watermark(), 0); // Still 0, seq 1 and 2 pending
    assert_eq!(tracker.current_lag(), 5);

    tracker.complete_event(seq1).await;
    assert_eq!(tracker.watermark(), 1); // Now 1, only seq 2 blocks
    assert_eq!(tracker.current_lag(), 4);

    tracker.complete_event(seq5).await;
    assert_eq!(tracker.watermark(), 1); // Still 1, seq 2 and 4 pending
    assert_eq!(tracker.current_lag(), 4);

    tracker.complete_event(seq2).await;
    assert_eq!(tracker.watermark(), 3); // Jumps to 3!
    assert_eq!(tracker.current_lag(), 2);

    tracker.complete_event(seq4).await;
    assert_eq!(tracker.watermark(), 5); // All complete
    assert_eq!(tracker.current_lag(), 0);
}

#[tokio::test]
async fn test_failed_events() {
    let tracker = LagTracker::new();

    let seq1 = tracker.assign_seq();
    let seq2 = tracker.assign_seq();
    let seq3 = tracker.assign_seq();

    tracker.start_event(seq1).await;
    tracker.start_event(seq2).await;
    tracker.start_event(seq3).await;

    // seq1 succeeds
    tracker.complete_event(seq1).await;
    assert_eq!(tracker.watermark(), 1);

    // seq2 fails
    tracker.fail_event(seq2, "test error").await;
    assert_eq!(tracker.watermark(), 2); // Watermark advances past failed event

    // seq3 succeeds
    tracker.complete_event(seq3).await;
    assert_eq!(tracker.watermark(), 3);
    assert_eq!(tracker.current_lag(), 0);

    // Check failed count
    let info = tracker.lag_info().await;
    assert_eq!(info.failed_count, 1);
}

#[tokio::test]
async fn test_lag_info() {
    let tracker = LagTracker::new();

    let seq1 = tracker.assign_seq();
    let seq2 = tracker.assign_seq();
    tracker.start_event(seq1).await;
    tracker.start_event(seq2).await;

    let info = tracker.lag_info().await;
    assert_eq!(info.total_assigned, 2);
    assert_eq!(info.pending_count, 2);
    assert_eq!(info.failed_count, 0);
    assert_eq!(info.lag, 2);
    assert_eq!(info.watermark, 0);
}

#[tokio::test]
async fn test_wait_for_zero_lag() {
    let tracker = Arc::new(LagTracker::new());

    let seq1 = tracker.assign_seq();
    tracker.start_event(seq1).await;

    // Spawn a task to complete the event after a delay
    let tracker_clone = Arc::clone(&tracker);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        tracker_clone.complete_event(seq1).await;
    });

    // Wait for zero lag
    let result = tracker.wait_for_zero_lag(Duration::from_secs(1)).await;
    assert!(result.is_ok());
    assert_eq!(tracker.current_lag(), 0);
}

#[tokio::test]
async fn test_wait_for_zero_lag_timeout() {
    let tracker = LagTracker::new();

    let seq1 = tracker.assign_seq();
    tracker.start_event(seq1).await;

    // Don't complete the event, expect timeout
    let result = tracker.wait_for_zero_lag(Duration::from_millis(50)).await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert_eq!(err.lag, 1);
}

#[tokio::test]
async fn test_reset() {
    let tracker = LagTracker::new();

    let seq1 = tracker.assign_seq();
    tracker.start_event(seq1).await;
    tracker.fail_event(seq1, "test").await;

    assert_eq!(tracker.lag_info().await.failed_count, 1);

    tracker.reset().await;

    let info = tracker.lag_info().await;
    assert_eq!(info.total_assigned, 0);
    assert_eq!(info.pending_count, 0);
    assert_eq!(info.failed_count, 0);
    assert_eq!(info.watermark, 0);
}

#[tokio::test]
async fn test_subscribe() {
    let tracker = LagTracker::new();
    let mut rx = tracker.subscribe();

    let seq1 = tracker.assign_seq();
    tracker.start_event(seq1).await;
    tracker.complete_event(seq1).await;

    // Should receive notification
    let lag = rx.recv().await.unwrap();
    assert_eq!(lag, 0);
}

#[tokio::test]
async fn test_cleanup_failed() {
    let tracker = LagTracker::new();

    // Create 10 failed events
    for _ in 0..10 {
        let seq = tracker.assign_seq();
        tracker.start_event(seq).await;
        tracker.fail_event(seq, "test error").await;
    }

    assert_eq!(tracker.failed_count().await, 10);

    // Cleanup keeping only 3
    tracker.cleanup_failed(3).await;
    assert_eq!(tracker.failed_count().await, 3);

    // Cleanup when under threshold should be no-op
    tracker.cleanup_failed(5).await;
    assert_eq!(tracker.failed_count().await, 3);
}

#[tokio::test]
async fn test_failed_count() {
    let tracker = LagTracker::new();
    assert_eq!(tracker.failed_count().await, 0);

    let seq = tracker.assign_seq();
    tracker.start_event(seq).await;
    tracker.fail_event(seq, "test").await;

    assert_eq!(tracker.failed_count().await, 1);
}
