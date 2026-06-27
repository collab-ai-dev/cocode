use coco_system_reminder::HookEvent;
use coco_system_reminder::HookEventKind;

use super::SyncHookEventBuffer;

#[tokio::test]
async fn drain_returns_pushed_events_in_fifo_order() {
    let buf = SyncHookEventBuffer::new();
    buf.push(HookEvent::Success {
        hook_name: "first".into(),
        hook_event: HookEventKind::SessionStart,
        content: "a".into(),
    })
    .await;
    buf.push(HookEvent::Success {
        hook_name: "second".into(),
        hook_event: HookEventKind::UserPromptSubmit,
        content: "b".into(),
    })
    .await;

    let drained = buf.drain().await;
    assert_eq!(drained.len(), 2);
    matches!(drained[0], HookEvent::Success { ref hook_name, .. } if hook_name == "first");
    matches!(drained[1], HookEvent::Success { ref hook_name, .. } if hook_name == "second");
}

#[tokio::test]
async fn drain_consumes_so_second_call_is_empty() {
    let buf = SyncHookEventBuffer::new();
    buf.push(HookEvent::AdditionalContext {
        hook_name: "h".into(),
        content: vec!["x".into()],
    })
    .await;
    assert_eq!(buf.drain().await.len(), 1);
    assert!(buf.drain().await.is_empty());
}

#[tokio::test]
async fn clones_share_storage() {
    let a = SyncHookEventBuffer::new();
    let b = a.clone();
    a.push(HookEvent::StoppedContinuation {
        hook_name: "h".into(),
        message: "stopped".into(),
    })
    .await;
    assert_eq!(b.drain().await.len(), 1);
    assert!(a.is_empty().await);
}
