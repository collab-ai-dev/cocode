use std::sync::Arc;
use std::time::Duration;

use coco_system_reminder::HookEvent;
use coco_system_reminder::HookEventKind;
use coco_system_reminder::HookEventsSource;

use super::CombinedHookEventsSource;
use crate::async_registry::AsyncHookRegistry;
use crate::sync_hook_buffer::SyncHookEventBuffer;

#[tokio::test]
async fn drain_returns_empty_when_no_pending_hooks() {
    let reg = AsyncHookRegistry::new();
    let events = reg.drain(None).await;
    assert!(events.is_empty());
}

#[tokio::test]
async fn drain_emits_async_response_for_completed_hooks() {
    let reg = AsyncHookRegistry::new();
    reg.register(
        "h1".into(),
        "my-hook".into(),
        "SessionStart".into(),
        Some(Duration::from_secs(30)),
    )
    .await;
    reg.update_output("h1", "hello from hook", "").await;
    reg.complete("h1", 0).await;

    let events = reg.drain(None).await;
    assert_eq!(events.len(), 1);
    match &events[0] {
        HookEvent::AsyncResponse {
            system_message,
            additional_context,
        } => {
            assert_eq!(system_message.as_deref(), Some("hello from hook"));
            assert!(additional_context.is_none());
        }
        other => panic!("expected AsyncResponse, got {other:?}"),
    }
}

#[tokio::test]
async fn drain_is_idempotent_after_first_call_delivers() {
    let reg = AsyncHookRegistry::new();
    reg.register("h".into(), "foo".into(), "UserPromptSubmit".into(), None)
        .await;
    reg.update_output("h", "out", "").await;
    reg.complete("h", 0).await;

    let first = reg.drain(None).await;
    assert_eq!(first.len(), 1);
    let second = reg.drain(None).await;
    assert!(second.is_empty(), "drain must mark responses delivered");
}

#[tokio::test]
async fn drain_packs_stderr_into_additional_context_with_hook_name() {
    let reg = AsyncHookRegistry::new();
    reg.register("h".into(), "linter".into(), "PostToolUse".into(), None)
        .await;
    reg.update_output("h", "stdout-text", "warning here").await;
    reg.complete("h", 0).await;

    let events = reg.drain(None).await;
    let HookEvent::AsyncResponse {
        additional_context, ..
    } = &events[0]
    else {
        panic!("expected AsyncResponse");
    };
    assert_eq!(additional_context.as_deref(), Some("[linter] warning here"));
}

#[tokio::test]
async fn combined_drain_orders_async_then_sync() {
    let async_reg = Arc::new(AsyncHookRegistry::new());
    async_reg
        .register("a".into(), "async-hook".into(), "SessionStart".into(), None)
        .await;
    async_reg.update_output("a", "async-out", "").await;
    async_reg.complete("a", 0).await;

    let sync = SyncHookEventBuffer::new();
    sync.push(HookEvent::Success {
        hook_name: "sync-hook".into(),
        hook_event: HookEventKind::SessionStart,
        content: "sync-out".into(),
    })
    .await;

    let combined = CombinedHookEventsSource::new(async_reg, sync.clone());
    let events = combined.drain(None).await;

    assert_eq!(events.len(), 2, "expected one async + one sync");
    matches!(events[0], HookEvent::AsyncResponse { .. });
    matches!(events[1], HookEvent::Success { .. });
    // Both buffers drained.
    assert!(sync.is_empty().await);
    assert!(combined.drain(None).await.is_empty());
}
