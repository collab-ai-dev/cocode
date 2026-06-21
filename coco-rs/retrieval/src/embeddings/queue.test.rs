use super::*;
use async_trait::async_trait;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;

/// Mock embedding provider for testing.
#[derive(Debug)]
struct MockProvider {
    dimension: i32,
    call_count: AtomicI32,
}

impl MockProvider {
    fn new() -> Self {
        Self {
            dimension: 128,
            call_count: AtomicI32::new(0),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn dimension(&self) -> i32 {
        self.dimension
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(vec![0.1; self.dimension as usize])
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(texts
            .iter()
            .map(|_| vec![0.1; self.dimension as usize])
            .collect())
    }
}

#[tokio::test]
async fn test_queue_creation() {
    let provider = Arc::new(MockProvider::new());
    let queue = EmbeddingQueue::new(provider.clone());
    assert_eq!(queue.workers, DEFAULT_WORKERS);
    assert_eq!(queue.batch_size, DEFAULT_BATCH_SIZE);
}

#[tokio::test]
async fn test_empty_requests() {
    let provider = Arc::new(MockProvider::new());
    let queue = EmbeddingQueue::new(provider);
    let results = queue.process_all(vec![]).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_single_request() {
    let provider = Arc::new(MockProvider::new());
    let queue = EmbeddingQueue::new(provider.clone()).with_batch_size(10);

    let requests = vec![EmbeddingRequest {
        id: "1".to_string(),
        text: "hello world".to_string(),
    }];

    let results = queue.process_all(requests).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].embedding.is_some());
    assert!(results[0].error.is_none());
}

#[tokio::test]
async fn test_multiple_batches() {
    let provider = Arc::new(MockProvider::new());
    let queue = EmbeddingQueue::new(provider.clone())
        .with_batch_size(2)
        .with_workers(2);

    let requests: Vec<EmbeddingRequest> = (0..5)
        .map(|i| EmbeddingRequest {
            id: i.to_string(),
            text: format!("text {i}"),
        })
        .collect();

    let results = queue.process_all(requests).await.unwrap();
    assert_eq!(results.len(), 5);

    // All should have embeddings
    for result in &results {
        assert!(result.embedding.is_some());
    }

    // Should have made 3 batch calls (5 requests / 2 batch size = 3 batches)
    assert_eq!(provider.call_count.load(Ordering::SeqCst), 3);
}

/// Mock provider that fails N times before succeeding.
#[derive(Debug)]
struct FailingProvider {
    dimension: i32,
    fail_count: AtomicI32,
    max_fails: i32,
    single_call_count: AtomicI32,
}

impl FailingProvider {
    fn new(max_fails: i32) -> Self {
        Self {
            dimension: 128,
            fail_count: AtomicI32::new(0),
            max_fails,
            single_call_count: AtomicI32::new(0),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for FailingProvider {
    fn name(&self) -> &str {
        "failing_mock"
    }

    fn dimension(&self) -> i32 {
        self.dimension
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        self.single_call_count.fetch_add(1, Ordering::SeqCst);
        Ok(vec![0.1; self.dimension as usize])
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let current = self.fail_count.fetch_add(1, Ordering::SeqCst);
        if current < self.max_fails {
            Err(crate::error::RetrievalErr::EmbeddingFailed {
                cause: "Simulated batch failure".to_string(),
            })
        } else {
            Ok(texts
                .iter()
                .map(|_| vec![0.1; self.dimension as usize])
                .collect())
        }
    }
}

#[tokio::test]
async fn test_retry_success_after_failures() {
    // Provider fails 2 times, then succeeds on 3rd attempt
    let provider = Arc::new(FailingProvider::new(2));
    let queue = EmbeddingQueue::new(provider.clone())
        .with_batch_size(10)
        .with_retry_config(RetryConfig {
            max_retries: 3,
            base_delay_ms: 1, // Fast for testing
            fallback_to_single: true,
        });

    let requests = vec![EmbeddingRequest {
        id: "1".to_string(),
        text: "test".to_string(),
    }];

    let results = queue.process_all(requests).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0].embedding.is_some(),
        "Should succeed after retries"
    );
    assert!(results[0].error.is_none());

    // Should have tried 3 times (2 fails + 1 success)
    assert_eq!(provider.fail_count.load(Ordering::SeqCst), 3);
    // No single-item fallback needed
    assert_eq!(provider.single_call_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_fallback_to_single_item() {
    // Provider always fails batch, but single works
    let provider = Arc::new(FailingProvider::new(100)); // Will always fail batch
    let queue = EmbeddingQueue::new(provider.clone())
        .with_batch_size(10)
        .with_retry_config(RetryConfig {
            max_retries: 2,
            base_delay_ms: 1,
            fallback_to_single: true,
        });

    let requests = vec![
        EmbeddingRequest {
            id: "1".to_string(),
            text: "test1".to_string(),
        },
        EmbeddingRequest {
            id: "2".to_string(),
            text: "test2".to_string(),
        },
    ];

    let results = queue.process_all(requests).await.unwrap();
    assert_eq!(results.len(), 2);

    // All should succeed via single-item fallback
    for result in &results {
        assert!(result.embedding.is_some(), "Should succeed via fallback");
        assert!(result.error.is_none());
    }

    // Batch tried 3 times (initial + 2 retries)
    assert_eq!(provider.fail_count.load(Ordering::SeqCst), 3);
    // Single-item fallback should have processed 2 items
    assert_eq!(provider.single_call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_no_fallback_returns_errors() {
    // Provider always fails, fallback disabled
    let provider = Arc::new(FailingProvider::new(100));
    let queue = EmbeddingQueue::new(provider.clone())
        .with_batch_size(10)
        .with_retry_config(RetryConfig {
            max_retries: 1,
            base_delay_ms: 1,
            fallback_to_single: false, // Disabled
        });

    let requests = vec![EmbeddingRequest {
        id: "1".to_string(),
        text: "test".to_string(),
    }];

    let results = queue.process_all(requests).await.unwrap();
    assert_eq!(results.len(), 1);

    // Should fail with error
    assert!(results[0].embedding.is_none());
    assert!(results[0].error.is_some());
    assert!(
        results[0]
            .error
            .as_ref()
            .unwrap()
            .contains("Simulated batch failure")
    );

    // No single-item fallback
    assert_eq!(provider.single_call_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_retry_config_builder() {
    let provider = Arc::new(MockProvider::new());
    let config = RetryConfig {
        max_retries: 5,
        base_delay_ms: 200,
        fallback_to_single: false,
    };
    let queue = EmbeddingQueue::new(provider).with_retry_config(config.clone());
    assert_eq!(queue.retry_config, config);
}
