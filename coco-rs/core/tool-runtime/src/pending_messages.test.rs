use super::*;

#[tokio::test]
async fn push_then_drain_returns_fifo_order() {
    let store = InMemoryPendingMessageStore::new();
    store
        .push(
            "agent-a",
            PendingMessage {
                from: "lead".into(),
                text: "first".into(),
            },
        )
        .await;
    store
        .push(
            "agent-a",
            PendingMessage {
                from: "lead".into(),
                text: "second".into(),
            },
        )
        .await;
    let drained = store.drain("agent-a").await;
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].text, "first");
    assert_eq!(drained[1].text, "second");
}

#[tokio::test]
async fn drain_clears_queue() {
    let store = InMemoryPendingMessageStore::new();
    store
        .push(
            "agent-a",
            PendingMessage {
                from: "lead".into(),
                text: "hi".into(),
            },
        )
        .await;
    let first = store.drain("agent-a").await;
    assert_eq!(first.len(), 1);
    let second = store.drain("agent-a").await;
    assert!(second.is_empty(), "drain must clear the queue");
}

#[tokio::test]
async fn peek_does_not_clear_queue() {
    let store = InMemoryPendingMessageStore::new();
    store
        .push(
            "agent-a",
            PendingMessage {
                from: "lead".into(),
                text: "hi".into(),
            },
        )
        .await;
    let first = store.peek("agent-a").await;
    assert_eq!(first.len(), 1);
    let second = store.peek("agent-a").await;
    assert_eq!(second.len(), 1, "peek must not clear the queue");
    let drained = store.drain("agent-a").await;
    assert_eq!(drained.len(), 1);
}

#[tokio::test]
async fn drain_unknown_recipient_returns_empty() {
    let store = InMemoryPendingMessageStore::new();
    assert!(store.drain("ghost").await.is_empty());
}

#[tokio::test]
async fn no_op_store_drains_empty() {
    let store = NoOpPendingMessageStore;
    store
        .push(
            "agent-a",
            PendingMessage {
                from: "lead".into(),
                text: "hi".into(),
            },
        )
        .await;
    assert!(store.drain("agent-a").await.is_empty());
}
