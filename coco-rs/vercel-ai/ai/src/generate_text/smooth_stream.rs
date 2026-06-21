//! Smooth streaming utilities.
//!
//! This module provides utilities for creating smooth streaming output
//! by controlling the rate of text emission.

use futures::Stream;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;
use std::time::Instant;

/// Configuration for smooth streaming.
#[derive(Debug, Clone)]
pub struct SmoothStreamConfig {
    /// Minimum delay between emitting chunks.
    pub min_chunk_delay: Duration,
    /// Maximum chunk size before splitting.
    pub max_chunk_size: usize,
}

impl Default for SmoothStreamConfig {
    fn default() -> Self {
        Self {
            min_chunk_delay: Duration::from_millis(10),
            max_chunk_size: 10,
        }
    }
}

/// A stream wrapper that smooths output by controlling emission rate.
pub struct SmoothStream<S> {
    inner: S,
    config: SmoothStreamConfig,
    buffer: String,
    last_emit: Option<Instant>,
}

impl<S> SmoothStream<S> {
    /// Create a new smooth stream.
    pub fn new(inner: S, config: SmoothStreamConfig) -> Self {
        Self {
            inner,
            config,
            buffer: String::new(),
            last_emit: None,
        }
    }
}

impl<S> Stream for SmoothStream<S>
where
    S: Stream<Item = String> + Unpin,
{
    type Item = String;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Check if we should emit from buffer
        if !self.buffer.is_empty()
            && let Some(last_emit) = self.last_emit
        {
            let elapsed = last_emit.elapsed();
            if elapsed >= self.config.min_chunk_delay {
                let chunk_size = self.config.max_chunk_size.min(self.buffer.len());
                let chunk = self.buffer.drain(..chunk_size).collect();
                self.last_emit = Some(Instant::now());
                return Poll::Ready(Some(chunk));
            }
        }

        // Poll inner stream
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(text)) => {
                self.buffer.push_str(&text);
                if self.last_emit.is_none() {
                    self.last_emit = Some(Instant::now());
                }

                // Try to emit immediately if delay has passed
                if self
                    .last_emit
                    .is_none_or(|t| t.elapsed() >= self.config.min_chunk_delay)
                {
                    let chunk_size = self.config.max_chunk_size.min(self.buffer.len());
                    if chunk_size > 0 {
                        let chunk = self.buffer.drain(..chunk_size).collect();
                        self.last_emit = Some(Instant::now());
                        return Poll::Ready(Some(chunk));
                    }
                }

                // Schedule another poll
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Ready(None) => {
                // Emit remaining buffer
                if !self.buffer.is_empty() {
                    let chunk = self.buffer.drain(..).collect();
                    Poll::Ready(Some(chunk))
                } else {
                    Poll::Ready(None)
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Create a smooth stream from an iterator.
pub fn smooth_stream_iter(
    iter: Vec<String>,
    config: SmoothStreamConfig,
) -> SmoothStream<futures::stream::Iter<std::vec::IntoIter<String>>> {
    SmoothStream::new(futures::stream::iter(iter), config)
}

#[cfg(test)]
#[path = "smooth_stream.test.rs"]
mod tests;
