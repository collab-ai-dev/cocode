//! Stream health metrics accumulated while processing stream parts.

use std::time::Duration;

use tokio::time::Instant;
use vercel_ai_provider::language_model::v4::stream::LanguageModelV4StreamPart;

/// Health metrics observed while consuming a stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamMetrics {
    /// Time to first content-bearing stream part in milliseconds.
    pub ttft_ms: Option<i64>,
    /// Number of gaps between stream items that exceeded the stall threshold.
    pub stall_count: i32,
    /// Sum of detected stall gaps in milliseconds.
    pub total_stall_ms: i64,
}

pub(super) struct StreamMetricsTracker {
    start_time: Instant,
    first_content_time: Option<Instant>,
    last_item_time: Option<Instant>,
    stall_threshold: Duration,
    stall_count: i32,
    total_stall_ms: i64,
}

impl StreamMetricsTracker {
    pub fn new(stall_threshold: Duration) -> Self {
        Self {
            start_time: Instant::now(),
            first_content_time: None,
            last_item_time: None,
            stall_threshold,
            stall_count: 0,
            total_stall_ms: 0,
        }
    }

    pub fn record_item(&mut self, part: Option<&LanguageModelV4StreamPart>) {
        let now = Instant::now();
        if let Some(last) = self.last_item_time {
            let gap = now.duration_since(last);
            if gap > self.stall_threshold {
                self.stall_count += 1;
                self.total_stall_ms += gap.as_millis() as i64;
            }
        }
        self.last_item_time = Some(now);

        if self.first_content_time.is_none() && part.is_some_and(is_first_content_candidate) {
            self.first_content_time = Some(now);
        }
    }

    pub fn snapshot(&self) -> StreamMetrics {
        StreamMetrics {
            ttft_ms: self
                .first_content_time
                .map(|t| t.duration_since(self.start_time).as_millis() as i64),
            stall_count: self.stall_count,
            total_stall_ms: self.total_stall_ms,
        }
    }
}

fn is_first_content_candidate(part: &LanguageModelV4StreamPart) -> bool {
    matches!(
        part,
        LanguageModelV4StreamPart::TextDelta { .. }
            | LanguageModelV4StreamPart::ReasoningDelta { .. }
            | LanguageModelV4StreamPart::ToolInputStart { .. }
    )
}
