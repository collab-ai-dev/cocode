use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::Readiness;
use super::ReadinessFlag;
use super::Token;
use super::errors::ReadinessError;
use assert_matches::assert_matches;

#[tokio::test]
async fn subscribe_and_mark_ready_roundtrip() -> Result<(), ReadinessError> {
    let flag = ReadinessFlag::new();
    let token = flag.subscribe().await?;

    assert!(flag.mark_ready(token).await?);
    assert!(flag.is_ready());
    Ok(())
}

#[tokio::test]
async fn subscribe_after_ready_returns_none() -> Result<(), ReadinessError> {
    let flag = ReadinessFlag::new();
    let token = flag.subscribe().await?;
    assert!(flag.mark_ready(token).await?);

    assert!(flag.subscribe().await.is_err());
    Ok(())
}

#[tokio::test]
async fn mark_ready_rejects_unknown_token() -> Result<(), ReadinessError> {
    let flag = ReadinessFlag::new();
    assert!(!flag.mark_ready(Token(42)).await?);
    assert!(!flag.load_ready());
    assert!(flag.is_ready());
    Ok(())
}

#[tokio::test]
async fn wait_ready_unblocks_after_mark_ready() -> Result<(), ReadinessError> {
    let flag = Arc::new(ReadinessFlag::new());
    let token = flag.subscribe().await?;

    let waiter = {
        let flag = Arc::clone(&flag);
        tokio::spawn(async move {
            flag.wait_ready().await;
        })
    };

    assert!(flag.mark_ready(token).await?);
    waiter.await.expect("waiting task should not panic");
    Ok(())
}

#[tokio::test]
async fn mark_ready_twice_uses_single_token() -> Result<(), ReadinessError> {
    let flag = ReadinessFlag::new();
    let token = flag.subscribe().await?;

    assert!(flag.mark_ready(token).await?);
    assert!(!flag.mark_ready(token).await?);
    Ok(())
}

#[tokio::test]
async fn is_ready_without_subscribers_marks_flag_ready() -> Result<(), ReadinessError> {
    let flag = ReadinessFlag::new();

    assert!(flag.is_ready());
    assert!(flag.is_ready());
    assert_matches!(
        flag.subscribe().await,
        Err(ReadinessError::FlagAlreadyReady)
    );
    Ok(())
}

#[tokio::test]
async fn subscribe_returns_error_when_lock_is_held() {
    let flag = ReadinessFlag::new();
    let _guard = flag
        .tokens
        .try_lock()
        .expect("initial lock acquisition should succeed");

    let err = flag
        .subscribe()
        .await
        .expect_err("contended subscribe should report a lock failure");
    assert_matches!(err, ReadinessError::TokenLockFailed);
}

#[tokio::test]
async fn subscribe_skips_zero_token() -> Result<(), ReadinessError> {
    let flag = ReadinessFlag::new();
    flag.next_id.store(0, Ordering::Relaxed);

    let token = flag.subscribe().await?;
    assert_ne!(token, Token(0));
    assert!(flag.mark_ready(token).await?);
    Ok(())
}

#[tokio::test]
async fn subscribe_avoids_duplicate_tokens() -> Result<(), ReadinessError> {
    let flag = ReadinessFlag::new();
    let token = flag.subscribe().await?;
    flag.next_id.store(token.0, Ordering::Relaxed);

    let token2 = flag.subscribe().await?;
    assert_ne!(token2, token);
    Ok(())
}
