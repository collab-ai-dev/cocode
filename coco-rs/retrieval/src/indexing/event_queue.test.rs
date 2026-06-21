use super::*;
use std::path::Path;

#[tokio::test]
async fn test_push_and_pop() {
    let queue = new_watch_event_queue(16);

    queue
        .push_simple(PathBuf::from("file1.rs"), WatchEventKind::Changed)
        .await;
    queue
        .push_simple(PathBuf::from("file2.rs"), WatchEventKind::Changed)
        .await;

    assert_eq!(queue.len().await, 2);

    let (path, event) = queue.pop().await.unwrap();
    assert!(path == Path::new("file1.rs") || path == Path::new("file2.rs"));
    assert_eq!(event.data, WatchEventKind::Changed);

    assert_eq!(queue.len().await, 1);
}

#[tokio::test]
async fn test_dedup_same_path() {
    let queue = new_watch_event_queue(16);

    // Same path, multiple events - should deduplicate
    queue
        .push_simple(PathBuf::from("file.rs"), WatchEventKind::Changed)
        .await;
    queue
        .push_simple(PathBuf::from("file.rs"), WatchEventKind::Changed)
        .await;

    // Should only have one event (deduplicated by path)
    assert_eq!(queue.len().await, 1);

    let (path, event) = queue.pop().await.unwrap();
    assert_eq!(path, PathBuf::from("file.rs"));
    assert_eq!(event.data, WatchEventKind::Changed);
}

#[tokio::test]
async fn test_tracked_event_with_batch_id() {
    let queue = new_watch_event_queue(16);
    let batch_id = BatchId::new();

    let event = TrackedEvent::new(
        WatchEventKind::Changed,
        Some(batch_id.clone()),
        42,
        "trace-123".to_string(),
    );

    queue.push(PathBuf::from("file.rs"), event).await;

    let (_, popped) = queue.pop().await.unwrap();
    assert_eq!(popped.batch_ids.len(), 1);
    assert_eq!(popped.batch_ids[0].as_str(), batch_id.as_str());
    assert_eq!(popped.seq, 42);
    assert_eq!(popped.trace_id, "trace-123");
}

#[tokio::test]
async fn test_batch_id_preserved_on_merge() {
    let queue = new_watch_event_queue(16);
    let batch_id = BatchId::new();

    // First event without batch_id
    queue
        .push_simple(PathBuf::from("file.rs"), WatchEventKind::Changed)
        .await;

    // Second event with batch_id
    let event = TrackedEvent::new(
        WatchEventKind::Changed,
        Some(batch_id.clone()),
        1,
        "trace".to_string(),
    );
    queue.push(PathBuf::from("file.rs"), event).await;

    let (_, popped) = queue.pop().await.unwrap();
    // batch_id should be preserved
    assert_eq!(popped.batch_ids.len(), 1);
    assert_eq!(popped.batch_ids[0].as_str(), batch_id.as_str());
}

#[tokio::test]
async fn test_multiple_batch_ids_preserved_on_merge() {
    let queue = new_watch_event_queue(16);
    let batch_id_1 = BatchId::new();
    let batch_id_2 = BatchId::new();

    // First event with batch_id_1
    let event1 = TrackedEvent::new(
        WatchEventKind::Changed,
        Some(batch_id_1.clone()),
        1,
        "trace-1".to_string(),
    );
    queue.push(PathBuf::from("file.rs"), event1).await;

    // Second event with batch_id_2 for same file - should merge
    let event2 = TrackedEvent::new(
        WatchEventKind::Changed,
        Some(batch_id_2.clone()),
        2,
        "trace-2".to_string(),
    );
    queue.push(PathBuf::from("file.rs"), event2).await;

    // Both batch_ids should be preserved
    let (_, popped) = queue.pop().await.unwrap();
    assert_eq!(popped.batch_ids.len(), 2);
    assert!(
        popped
            .batch_ids
            .iter()
            .any(|b| b.as_str() == batch_id_1.as_str())
    );
    assert!(
        popped
            .batch_ids
            .iter()
            .any(|b| b.as_str() == batch_id_2.as_str())
    );
}

#[tokio::test]
async fn test_requeue() {
    let queue = new_watch_event_queue(16);

    queue
        .push_simple(PathBuf::from("file.rs"), WatchEventKind::Changed)
        .await;
    let (path, event) = queue.pop().await.unwrap();

    // Requeue the event
    queue.requeue(path.clone(), event).await;

    assert_eq!(queue.len().await, 1);
    let (p, e) = queue.pop().await.unwrap();
    assert_eq!(p, path);
    assert_eq!(e.data, WatchEventKind::Changed);
}

#[tokio::test]
async fn test_empty_queue() {
    let queue = new_watch_event_queue(16);
    assert!(queue.is_empty().await);
    assert!(queue.pop().await.is_none());
}

#[tokio::test]
async fn test_subscribe_notification() {
    let queue = new_watch_event_queue(16);
    let mut rx = queue.subscribe();

    // Push should notify
    queue
        .push_simple(PathBuf::from("file.rs"), WatchEventKind::Changed)
        .await;

    // Should receive notification
    let result = rx.try_recv();
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_tag_event_queue() {
    let queue = new_tag_event_queue(16);

    // Multiple events for same file should deduplicate
    queue
        .push_simple(PathBuf::from("file.rs"), TagEventKind::Changed)
        .await;
    queue
        .push_simple(PathBuf::from("file.rs"), TagEventKind::Changed)
        .await;

    assert_eq!(queue.len().await, 1);
    let (_, event) = queue.pop().await.unwrap();
    assert_eq!(event.data, TagEventKind::Changed);
}

#[tokio::test]
async fn test_pending_keys() {
    let queue = new_watch_event_queue(16);

    queue
        .push_simple(PathBuf::from("a.rs"), WatchEventKind::Changed)
        .await;
    queue
        .push_simple(PathBuf::from("b.rs"), WatchEventKind::Changed)
        .await;

    let keys = queue.pending_keys().await;
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&PathBuf::from("a.rs")));
    assert!(keys.contains(&PathBuf::from("b.rs")));
}

#[tokio::test]
async fn test_merged_seqs_tracked() {
    let queue = new_watch_event_queue(16);

    // First event with seq=1
    let event1 = TrackedEvent::new(WatchEventKind::Changed, None, 1, "trace-1".to_string());
    queue.push(PathBuf::from("file.rs"), event1).await;

    // Second event with seq=2 for same file - should merge
    let event2 = TrackedEvent::new(WatchEventKind::Changed, None, 2, "trace-2".to_string());
    queue.push(PathBuf::from("file.rs"), event2).await;

    // Third event with seq=3 for same file - should merge again
    let event3 = TrackedEvent::new(WatchEventKind::Changed, None, 3, "trace-3".to_string());
    queue.push(PathBuf::from("file.rs"), event3).await;

    // Should have only one event with seq=3 and merged_seqs=[1, 2]
    assert_eq!(queue.len().await, 1);
    let (_, popped) = queue.pop().await.unwrap();
    assert_eq!(popped.seq, 3);
    assert_eq!(popped.merged_seqs.len(), 2);
    assert!(popped.merged_seqs.contains(&1));
    assert!(popped.merged_seqs.contains(&2));
}

#[tokio::test]
async fn test_merged_seqs_preserves_batch_id() {
    let queue = new_watch_event_queue(16);
    let batch_id = BatchId::new();

    // First event with batch_id and seq=1
    let event1 = TrackedEvent::new(
        WatchEventKind::Changed,
        Some(batch_id.clone()),
        1,
        "trace-1".to_string(),
    );
    queue.push(PathBuf::from("file.rs"), event1).await;

    // Second event without batch_id but seq=2
    let event2 = TrackedEvent::new(WatchEventKind::Changed, None, 2, "trace-2".to_string());
    queue.push(PathBuf::from("file.rs"), event2).await;

    // Should preserve batch_id from first event and track seq=1 as merged
    let (_, popped) = queue.pop().await.unwrap();
    assert_eq!(popped.seq, 2);
    assert_eq!(popped.batch_ids.len(), 1);
    assert_eq!(popped.batch_ids[0].as_str(), batch_id.as_str());
    assert_eq!(popped.merged_seqs, vec![1]);
}
