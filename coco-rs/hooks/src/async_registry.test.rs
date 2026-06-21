use std::time::Duration;

use super::*;

#[tokio::test]
async fn test_register_and_complete() {
    let registry = AsyncHookRegistry::new();

    registry
        .register(
            "hook-1".to_string(),
            "echo test".to_string(),
            "PreToolUse".to_string(),
            None,
        )
        .await;

    assert_eq!(registry.pending_count().await, 1);

    // Not yet completed — no responses
    let responses = registry.collect_responses().await;
    assert!(responses.is_empty());

    // Complete the hook
    registry.complete("hook-1", 0).await;

    let responses = registry.collect_responses().await;
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].hook_id, "hook-1");
    assert_eq!(responses[0].exit_code, 0);
    assert!(!responses[0].timed_out);

    // Already delivered — no new responses
    let responses = registry.collect_responses().await;
    assert!(responses.is_empty());
}

#[tokio::test]
async fn test_timeout_detection() {
    let registry = AsyncHookRegistry::new();

    registry
        .register(
            "hook-timeout".to_string(),
            "slow cmd".to_string(),
            "PostToolUse".to_string(),
            Some(Duration::from_millis(10)),
        )
        .await;

    // Wait for timeout
    tokio::time::sleep(Duration::from_millis(50)).await;

    let responses = registry.collect_responses().await;
    assert_eq!(responses.len(), 1);
    assert!(responses[0].timed_out);
    assert_eq!(responses[0].exit_code, -1);
}

#[tokio::test]
async fn test_update_output() {
    let registry = AsyncHookRegistry::new();

    registry
        .register(
            "hook-out".to_string(),
            "cmd".to_string(),
            "Setup".to_string(),
            None,
        )
        .await;

    registry
        .update_output("hook-out", "hello world", "some warning")
        .await;
    registry.complete("hook-out", 0).await;

    let responses = registry.collect_responses().await;
    assert_eq!(responses[0].stdout, "hello world");
    assert_eq!(responses[0].stderr, "some warning");
}

#[tokio::test]
async fn test_cleanup_delivered() {
    let registry = AsyncHookRegistry::new();

    registry
        .register(
            "hook-a".to_string(),
            "cmd a".to_string(),
            "PreToolUse".to_string(),
            None,
        )
        .await;
    registry
        .register(
            "hook-b".to_string(),
            "cmd b".to_string(),
            "PreToolUse".to_string(),
            None,
        )
        .await;

    registry.complete("hook-a", 0).await;
    let _ = registry.collect_responses().await;

    // hook-a is delivered, hook-b is still pending
    assert_eq!(registry.pending_count().await, 1);

    registry.cleanup_delivered().await;

    // After cleanup, only hook-b remains in the map
    assert_eq!(registry.pending_count().await, 1);
}

#[tokio::test]
async fn test_finalize_all() {
    let registry = AsyncHookRegistry::new();

    registry
        .register(
            "hook-1".to_string(),
            "cmd 1".to_string(),
            "Stop".to_string(),
            None,
        )
        .await;
    registry
        .register(
            "hook-2".to_string(),
            "cmd 2".to_string(),
            "Stop".to_string(),
            None,
        )
        .await;

    // Complete one, leave the other pending
    registry.complete("hook-1", 0).await;

    let responses = registry.finalize_all().await;
    assert_eq!(responses.len(), 2);

    // hook-1 completed normally
    let r1 = responses.iter().find(|r| r.hook_id == "hook-1").unwrap();
    assert_eq!(r1.exit_code, 0);
    assert!(!r1.timed_out);

    // hook-2 was still pending — forced finalization
    let r2 = responses.iter().find(|r| r.hook_id == "hook-2").unwrap();
    assert_eq!(r2.exit_code, -1);
    assert!(r2.timed_out);
}

#[tokio::test]
async fn test_multiple_hooks_independent() {
    let registry = AsyncHookRegistry::new();

    registry
        .register("h1".to_string(), "c1".to_string(), "A".to_string(), None)
        .await;
    registry
        .register("h2".to_string(), "c2".to_string(), "B".to_string(), None)
        .await;
    registry
        .register("h3".to_string(), "c3".to_string(), "C".to_string(), None)
        .await;

    assert_eq!(registry.pending_count().await, 3);

    registry.complete("h2", 42).await;

    let responses = registry.collect_responses().await;
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].hook_id, "h2");
    assert_eq!(responses[0].exit_code, 42);

    assert_eq!(registry.pending_count().await, 2);
}
