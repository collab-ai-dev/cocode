use super::*;

#[tokio::test]
async fn register_then_resolve_delivers_payload() {
    let map: PendingMap<i32> = PendingMap::new();
    let rx = map.register("req-1".into()).await;
    let outcome = map.resolve("req-1", 42).await;
    assert_eq!(outcome, ResolveOutcome::Delivered);
    assert_eq!(rx.await.unwrap(), 42);
}

#[tokio::test]
async fn resolve_missing_returns_not_found() {
    let map: PendingMap<i32> = PendingMap::new();
    let outcome = map.resolve("missing", 1).await;
    assert_eq!(outcome, ResolveOutcome::NotFound);
}

#[tokio::test]
async fn resolve_after_receiver_dropped_reports_dropped() {
    let map: PendingMap<i32> = PendingMap::new();
    let rx = map.register("req-1".into()).await;
    drop(rx);
    let outcome = map.resolve("req-1", 7).await;
    assert_eq!(outcome, ResolveOutcome::ReceiverDropped);
}

#[tokio::test]
async fn remove_without_deliver_returns_true_then_false() {
    let map: PendingMap<String> = PendingMap::new();
    let _rx = map.register("req-1".into()).await;
    assert!(map.remove("req-1").await);
    // Second remove sees the slot gone.
    assert!(!map.remove("req-1").await);
}

#[tokio::test]
async fn multiple_pending_entries_isolated() {
    let map: PendingMap<String> = PendingMap::new();
    let rx_a = map.register("a".into()).await;
    let rx_b = map.register("b".into()).await;
    assert_eq!(
        map.resolve("a", "hello".into()).await,
        ResolveOutcome::Delivered
    );
    assert_eq!(rx_a.await.unwrap(), "hello");
    assert_eq!(
        map.resolve("b", "world".into()).await,
        ResolveOutcome::Delivered
    );
    assert_eq!(rx_b.await.unwrap(), "world");
}
