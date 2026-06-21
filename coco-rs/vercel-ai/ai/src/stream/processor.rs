//! StreamProcessor: thin adapter that adds idle-timeout + health metrics
//! around a raw `Stream<LanguageModelV4StreamPart>`.
//!
//! Content accumulation is deliberately not done here. Different consumers
//! want different accumulators (coco-inference needs per-part
//! `provider_metadata` fidelity for round-tripping signatures; a telemetry
//! pipeline only wants `Finish`), so baking one into the SDK layer would
//! privilege one consumer's view. Each consumer owns its own state.

use std::pin::Pin;
use std::time::Duration;

use futures::Stream;
use futures::StreamExt;
use vercel_ai_provider::errors::AISdkError;
use vercel_ai_provider::language_model::v4::stream::LanguageModelV4StreamPart;

use super::metrics::StreamMetrics;
use super::metrics::StreamMetricsTracker;

const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_STALL_THRESHOLD: Duration = Duration::from_secs(30);

type PartStream =
    Pin<Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send + 'static>>;

/// Configuration for the stream processor.
#[derive(Debug, Clone)]
pub struct StreamProcessorConfig {
    /// Timeout for idle streams (no events received). `None` disables idle
    /// timeout checks. Default: 60s.
    pub idle_timeout: Option<Duration>,
    /// Gap between stream items that is counted as a stall. Default: 30s.
    pub stall_threshold: Duration,
}

impl Default for StreamProcessorConfig {
    fn default() -> Self {
        Self {
            idle_timeout: Some(DEFAULT_IDLE_TIMEOUT),
            stall_threshold: DEFAULT_STALL_THRESHOLD,
        }
    }
}

impl StreamProcessorConfig {
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = Some(timeout);
        self
    }

    pub fn without_idle_timeout(mut self) -> Self {
        self.idle_timeout = None;
        self
    }

    pub fn with_stall_threshold(mut self, threshold: Duration) -> Self {
        self.stall_threshold = threshold;
        self
    }
}

/// Wraps a stream of `LanguageModelV4StreamPart` with idle-timeout enforcement
/// and health metrics (ttft, stall_count, total_stall_ms). The wrapper does
/// not accumulate content — callers consume parts via [`StreamProcessor::next`]
/// and build their own state.
pub struct StreamProcessor {
    stream: PartStream,
    config: StreamProcessorConfig,
    metrics: StreamMetricsTracker,
}

impl StreamProcessor {
    pub fn from_stream(stream: PartStream) -> Self {
        Self::from_stream_with_config(stream, StreamProcessorConfig::default())
    }

    pub fn from_stream_with_config(stream: PartStream, config: StreamProcessorConfig) -> Self {
        let stall_threshold = config.stall_threshold;
        Self {
            stream,
            config,
            metrics: StreamMetricsTracker::new(stall_threshold),
        }
    }

    /// Health metrics observed so far.
    pub fn metrics(&self) -> StreamMetrics {
        self.metrics.snapshot()
    }

    /// Pull the next part, applying the configured idle timeout and updating
    /// health metrics. Returns `None` when the stream ends.
    pub async fn next(&mut self) -> Option<Result<LanguageModelV4StreamPart, AISdkError>> {
        let next_item = if let Some(timeout) = self.config.idle_timeout {
            match tokio::time::timeout(timeout, self.stream.next()).await {
                Ok(item) => item,
                Err(_) => {
                    return Some(Err(AISdkError::new(format!(
                        "Stream idle timeout after {}s",
                        timeout.as_secs()
                    ))));
                }
            }
        } else {
            self.stream.next().await
        };

        match next_item {
            Some(Ok(part)) => {
                self.metrics.record_item(Some(&part));
                Some(Ok(part))
            }
            Some(Err(e)) => {
                self.metrics.record_item(None);
                Some(Err(e))
            }
            None => None,
        }
    }
}

impl std::fmt::Debug for StreamProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamProcessor")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "processor.test.rs"]
mod tests;
