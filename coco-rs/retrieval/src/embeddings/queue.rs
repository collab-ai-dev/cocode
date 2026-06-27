//! Concurrent embedding queue for batch processing.
//!
//! Processes embedding requests using multiple workers with batching
//! for efficiency. Includes retry logic with exponential backoff and
//! fallback to single-item embedding on batch failure.

use std::sync::Arc;
use std::time::Duration;

use futures::stream::FuturesUnordered;
use futures::stream::StreamExt;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::error::Result;
use crate::traits::EmbeddingProvider;

/// Default number of concurrent workers.
const DEFAULT_WORKERS: i32 = 4;
/// Default batch size for embedding requests.
const DEFAULT_BATCH_SIZE: i32 = 100;

/// Retry configuration for embedding requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetryConfig {
    /// Maximum number of retry attempts for batch embedding.
    #[serde(default = "default_max_retries")]
    pub max_retries: i32,

    /// Base delay in milliseconds for exponential backoff.
    #[serde(default = "default_base_delay_ms")]
    pub base_delay_ms: i32,

    /// Whether to fall back to single-item embedding when batch fails.
    #[serde(default = "default_fallback_to_single")]
    pub fallback_to_single: bool,
}

fn default_max_retries() -> i32 {
    3
}
fn default_base_delay_ms() -> i32 {
    100
}
fn default_fallback_to_single() -> bool {
    true
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            base_delay_ms: default_base_delay_ms(),
            fallback_to_single: default_fallback_to_single(),
        }
    }
}

/// Request to embed a chunk of text.
#[derive(Debug, Clone)]
pub struct EmbeddingRequest {
    /// Unique identifier for the request.
    pub id: String,
    /// Text to embed.
    pub text: String,
}

/// Result of an embedding request.
#[derive(Debug, Clone)]
pub struct EmbeddingResult {
    /// Request ID.
    pub id: String,
    /// Embedding vector (None if failed).
    pub embedding: Option<Vec<f32>>,
    /// Error message if failed.
    pub error: Option<String>,
}

/// Concurrent embedding queue.
///
/// Processes embedding requests using multiple workers and batching.
/// Supports retry with exponential backoff and fallback to single-item embedding.
pub struct EmbeddingQueue {
    provider: Arc<dyn EmbeddingProvider>,
    workers: i32,
    batch_size: i32,
    retry_config: RetryConfig,
}

impl EmbeddingQueue {
    /// Create a new embedding queue.
    pub fn new(provider: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            provider,
            workers: DEFAULT_WORKERS,
            batch_size: DEFAULT_BATCH_SIZE,
            retry_config: RetryConfig::default(),
        }
    }

    /// Set the number of concurrent workers.
    pub fn with_workers(mut self, workers: i32) -> Self {
        self.workers = workers;
        self
    }

    /// Set the batch size.
    pub fn with_batch_size(mut self, batch_size: i32) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Set the retry configuration.
    pub fn with_retry_config(mut self, config: RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    /// Process a batch of embedding requests.
    ///
    /// Returns a receiver for results as they complete.
    /// Uses retry logic with exponential backoff and optional fallback
    /// to single-item embedding on batch failure.
    pub async fn process(
        &self,
        requests: Vec<EmbeddingRequest>,
    ) -> Result<mpsc::Receiver<EmbeddingResult>> {
        let (tx, rx) = mpsc::channel(requests.len().max(1));

        if requests.is_empty() {
            return Ok(rx);
        }

        // Chunk requests into batches
        let batches: Vec<Vec<EmbeddingRequest>> = requests
            .chunks(self.batch_size as usize)
            .map(|c| c.to_vec())
            .collect();

        let provider = self.provider.clone();
        let workers = self.workers;
        let retry_config = self.retry_config.clone();

        tokio::spawn(async move {
            // Process batches concurrently using FuturesUnordered
            let mut futures = FuturesUnordered::new();
            let mut batch_iter = batches.into_iter();
            // Start initial workers
            for batch in batch_iter.by_ref().take(workers as usize) {
                let p = provider.clone();
                let tx = tx.clone();
                let cfg = retry_config.clone();
                futures.push(tokio::spawn(async move {
                    process_batch_with_retry(p, batch, tx, cfg).await
                }));
            }

            // Process remaining batches as workers complete
            while let Some(result) = futures.next().await {
                // Log any errors but continue processing
                if let Err(e) = result {
                    tracing::warn!("Embedding batch task failed: {e}");
                }

                // Start next batch if available
                if let Some(batch) = batch_iter.next() {
                    let p = provider.clone();
                    let tx = tx.clone();
                    let cfg = retry_config.clone();
                    futures.push(tokio::spawn(async move {
                        process_batch_with_retry(p, batch, tx, cfg).await
                    }));
                }
            }
        });

        Ok(rx)
    }

    /// Process requests synchronously and return all results.
    pub async fn process_all(
        &self,
        requests: Vec<EmbeddingRequest>,
    ) -> Result<Vec<EmbeddingResult>> {
        let mut rx = self.process(requests.clone()).await?;
        let mut results = Vec::with_capacity(requests.len());

        while let Some(result) = rx.recv().await {
            results.push(result);
        }

        Ok(results)
    }
}

/// Process a batch with retry logic and exponential backoff.
///
/// On batch failure, retries with exponential backoff. If all retries fail
/// and `fallback_to_single` is enabled, processes items one by one.
async fn process_batch_with_retry(
    provider: Arc<dyn EmbeddingProvider>,
    batch: Vec<EmbeddingRequest>,
    tx: mpsc::Sender<EmbeddingResult>,
    config: RetryConfig,
) {
    let texts: Vec<String> = batch.iter().map(|r| r.text.clone()).collect();
    let ids: Vec<String> = batch.iter().map(|r| r.id.clone()).collect();

    // Try batch embedding with retries
    for attempt in 0..=config.max_retries {
        match provider.embed_batch(&texts).await {
            Ok(embeddings) => {
                // Success - send all results
                for (id, embedding) in ids.into_iter().zip(embeddings) {
                    let _ = tx
                        .send(EmbeddingResult {
                            id,
                            embedding: Some(embedding),
                            error: None,
                        })
                        .await;
                }
                return;
            }
            Err(e) => {
                if attempt < config.max_retries {
                    // Exponential backoff: base_delay * 2^attempt
                    let delay_ms = config.base_delay_ms as u64 * (1u64 << attempt as u64);
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_retries = config.max_retries,
                        delay_ms = delay_ms,
                        error = %e,
                        batch_size = texts.len(),
                        "Batch embedding failed, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                } else {
                    // All retries exhausted
                    tracing::error!(
                        attempts = config.max_retries + 1,
                        error = %e,
                        batch_size = texts.len(),
                        fallback = config.fallback_to_single,
                        "Batch embedding failed after all retries"
                    );

                    if config.fallback_to_single {
                        // Fallback: process items one by one
                        process_single_items(provider, batch, tx).await;
                    } else {
                        // No fallback - send errors for all
                        let error_msg = e.to_string();
                        for id in ids {
                            let _ = tx
                                .send(EmbeddingResult {
                                    id,
                                    embedding: None,
                                    error: Some(error_msg.clone()),
                                })
                                .await;
                        }
                    }
                    return;
                }
            }
        }
    }
}

/// Process items one by one as fallback when batch fails.
async fn process_single_items(
    provider: Arc<dyn EmbeddingProvider>,
    batch: Vec<EmbeddingRequest>,
    tx: mpsc::Sender<EmbeddingResult>,
) {
    let mut success_count = 0;
    let mut fail_count = 0;
    let total = batch.len();

    for request in batch {
        match provider.embed(&request.text).await {
            Ok(embedding) => {
                success_count += 1;
                let _ = tx
                    .send(EmbeddingResult {
                        id: request.id,
                        embedding: Some(embedding),
                        error: None,
                    })
                    .await;
            }
            Err(e) => {
                fail_count += 1;
                tracing::warn!(
                    id = %request.id,
                    error = %e,
                    "Single item embedding failed"
                );
                let _ = tx
                    .send(EmbeddingResult {
                        id: request.id,
                        embedding: None,
                        error: Some(e.to_string()),
                    })
                    .await;
            }
        }
    }

    tracing::info!(
        total = total,
        success = success_count,
        failed = fail_count,
        "Single-item fallback completed"
    );
}

#[cfg(test)]
#[path = "queue.test.rs"]
mod tests;
