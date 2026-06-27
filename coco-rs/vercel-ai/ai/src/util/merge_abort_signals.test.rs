use super::*;
use tokio::time::Duration;
use tokio::time::sleep;

#[tokio::test]
async fn test_merge_abort_signals_empty() {
    let merged = merge_abort_signals(&[]);
    assert!(!merged.is_cancelled());
}

#[tokio::test]
async fn test_merge_abort_signals_single() {
    let token = CancellationToken::new();
    let merged = merge_abort_signals(std::slice::from_ref(&token));

    assert!(!merged.is_cancelled());

    token.cancel();
    sleep(Duration::from_millis(10)).await;

    assert!(merged.is_cancelled());
}

#[tokio::test]
async fn test_merge_abort_signals_multiple() {
    let token1 = CancellationToken::new();
    let token2 = CancellationToken::new();
    let merged = merge_abort_signals(&[token1.clone(), token2.clone()]);

    assert!(!merged.is_cancelled());

    token1.cancel();
    sleep(Duration::from_millis(10)).await;

    assert!(merged.is_cancelled());
}

#[tokio::test]
async fn test_create_timeout_token() {
    let token = create_timeout_token(Duration::from_millis(50));

    assert!(!token.is_cancelled());

    sleep(Duration::from_millis(100)).await;

    assert!(token.is_cancelled());
}

#[tokio::test]
async fn test_merge_with_timeout() {
    let token = CancellationToken::new();
    let merged = merge_abort_signals_with_timeout(
        std::slice::from_ref(&token),
        Some(Duration::from_millis(50)),
    );

    assert!(!merged.is_cancelled());

    // Let timeout trigger
    sleep(Duration::from_millis(100)).await;

    assert!(merged.is_cancelled());
}

#[tokio::test]
async fn test_cancellation_manager() {
    let mut manager = CancellationManager::new();

    assert!(!manager.is_cancelled());

    let child = manager.child_token();
    assert!(!child.is_cancelled());

    manager.cancel();

    assert!(manager.is_cancelled());
    assert!(child.is_cancelled());
}

#[tokio::test]
async fn test_cancellation_manager_with_token() {
    let token = CancellationToken::new();
    let manager = CancellationManager::with_token(token.clone());

    token.cancel();

    assert!(manager.is_cancelled());
}
