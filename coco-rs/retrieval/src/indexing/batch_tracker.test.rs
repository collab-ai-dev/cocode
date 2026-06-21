use super::*;

#[tokio::test]
async fn test_batch_completion() {
    let tracker = BatchTracker::new();
    let batch_id = BatchId::from_str("test-batch-1");

    let rx = tracker.start_batch(batch_id.clone(), 3).await;

    // Mark events as complete
    tracker.mark_complete(&batch_id, true).await;
    tracker.mark_complete(&batch_id, true).await;
    tracker.mark_complete(&batch_id, true).await;

    // Should receive result
    let result = rx.await.unwrap();
    assert_eq!(result.completed, 3);
    assert_eq!(result.failed, 0);
}

#[tokio::test]
async fn test_batch_with_failures() {
    let tracker = BatchTracker::new();
    let batch_id = BatchId::from_str("test-batch-2");

    let rx = tracker.start_batch(batch_id.clone(), 3).await;

    // Mark events with mixed results
    tracker.mark_complete(&batch_id, true).await;
    tracker.mark_complete(&batch_id, false).await;
    tracker.mark_complete(&batch_id, true).await;

    let result = rx.await.unwrap();
    assert_eq!(result.completed, 2);
    assert_eq!(result.failed, 1);
}

#[tokio::test]
async fn test_progress_tracking() {
    let tracker = BatchTracker::new();
    let batch_id = BatchId::from_str("test-batch-3");

    let _rx = tracker.start_batch(batch_id.clone(), 5).await;

    // Check initial progress
    let (completed, failed, total) = tracker.progress(&batch_id).await.unwrap();
    assert_eq!(completed, 0);
    assert_eq!(failed, 0);
    assert_eq!(total, 5);

    // Mark some as complete
    tracker.mark_complete(&batch_id, true).await;
    tracker.mark_complete(&batch_id, false).await;

    let (completed, failed, total) = tracker.progress(&batch_id).await.unwrap();
    assert_eq!(completed, 1);
    assert_eq!(failed, 1);
    assert_eq!(total, 5);
}

#[tokio::test]
async fn test_multiple_batches() {
    let tracker = BatchTracker::new();
    let batch1 = BatchId::from_str("batch-1");
    let batch2 = BatchId::from_str("batch-2");

    let rx1 = tracker.start_batch(batch1.clone(), 2).await;
    let rx2 = tracker.start_batch(batch2.clone(), 2).await;

    assert_eq!(tracker.active_batch_count().await, 2);

    // Complete batch1
    tracker.mark_complete(&batch1, true).await;
    tracker.mark_complete(&batch1, true).await;

    let result1 = rx1.await.unwrap();
    assert_eq!(result1.completed, 2);
    assert_eq!(tracker.active_batch_count().await, 1);

    // Complete batch2
    tracker.mark_complete(&batch2, true).await;
    tracker.mark_complete(&batch2, true).await;

    let result2 = rx2.await.unwrap();
    assert_eq!(result2.completed, 2);
    assert_eq!(tracker.active_batch_count().await, 0);
}

#[tokio::test]
async fn test_cancel_batch() {
    let tracker = BatchTracker::new();
    let batch_id = BatchId::from_str("test-batch-cancel");

    let rx = tracker.start_batch(batch_id.clone(), 3).await;

    tracker.mark_complete(&batch_id, true).await;
    tracker.cancel_batch(&batch_id).await;

    // Receiver should get an error (sender dropped)
    assert!(rx.await.is_err());
    assert!(tracker.is_complete(&batch_id).await);
}
