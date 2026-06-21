//! Tests for retry.rs

use super::*;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

#[test]
fn test_retry_config_default() {
    let config = RetryConfig::default();
    assert_eq!(config.max_retries, 2);
    assert_eq!(config.initial_delay_ms, 1000);
    assert_eq!(config.max_delay_ms, 30000);
    assert_eq!(config.multiplier, 2.0);
}

#[test]
fn test_retry_config_builder() {
    let config = RetryConfig::new()
        .with_max_retries(5)
        .with_initial_delay_ms(500)
        .with_max_delay_ms(10000)
        .with_multiplier(1.5);

    assert_eq!(config.max_retries, 5);
    assert_eq!(config.initial_delay_ms, 500);
    assert_eq!(config.max_delay_ms, 10000);
    assert_eq!(config.multiplier, 1.5);
}

#[test]
fn test_delay_for_attempt() {
    let config = RetryConfig::new()
        .with_initial_delay_ms(1000)
        .with_multiplier(2.0)
        .with_max_delay_ms(10000);

    assert_eq!(config.delay_for_attempt(0), Duration::from_millis(1000));
    assert_eq!(config.delay_for_attempt(1), Duration::from_millis(2000));
    assert_eq!(config.delay_for_attempt(2), Duration::from_millis(4000));
    assert_eq!(config.delay_for_attempt(3), Duration::from_millis(8000));
    // Capped at max_delay_ms
    assert_eq!(config.delay_for_attempt(4), Duration::from_millis(10000));
}

#[derive(Debug)]
struct TestError {
    retryable: bool,
}

impl RetryableError for TestError {
    fn is_retryable(&self) -> bool {
        self.retryable
    }
}

#[tokio::test]
async fn test_with_retry_success_on_first_attempt() {
    let config = RetryConfig::new();
    let attempts = Arc::new(AtomicU32::new(0));

    let result = with_retry(config, None, || {
        let attempts = attempts.clone();
        async move {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok::<i32, TestError>(42)
        }
    })
    .await;

    assert_eq!(result.unwrap(), 42);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_with_retry_success_on_second_attempt() {
    let config = RetryConfig::new().with_initial_delay_ms(1);
    let attempts = Arc::new(AtomicU32::new(0));

    let result = with_retry(config, None, || {
        let attempts = attempts.clone();
        async move {
            let count = attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if count == 1 {
                Err(TestError { retryable: true })
            } else {
                Ok::<i32, TestError>(42)
            }
        }
    })
    .await;

    assert_eq!(result.unwrap(), 42);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_with_retry_non_retryable_error() {
    let config = RetryConfig::new();
    let attempts = Arc::new(AtomicU32::new(0));

    let result = with_retry(config, None, || {
        let attempts = attempts.clone();
        async move {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err::<i32, TestError>(TestError { retryable: false })
        }
    })
    .await;

    assert!(result.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_with_retry_max_retries_exceeded() {
    let config = RetryConfig::new()
        .with_max_retries(2)
        .with_initial_delay_ms(1);
    let attempts = Arc::new(AtomicU32::new(0));

    let result = with_retry(config, None, || {
        let attempts = attempts.clone();
        async move {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err::<i32, TestError>(TestError { retryable: true })
        }
    })
    .await;

    assert!(result.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 3); // 1 initial + 2 retries
}

#[tokio::test]
async fn test_with_retry_cancellation() {
    let config = RetryConfig::new();
    let token = CancellationToken::new();
    token.cancel();

    let attempts = Arc::new(AtomicU32::new(0));
    let result = with_retry(config, Some(token.clone()), || {
        let attempts = attempts.clone();
        async move {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err::<i32, TestError>(TestError { retryable: true })
        }
    })
    .await;

    // When cancelled before starting, should still execute once and return error
    assert!(result.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}
