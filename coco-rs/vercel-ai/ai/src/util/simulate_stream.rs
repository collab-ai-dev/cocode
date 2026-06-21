//! Simulate stream for testing.
//!
//! This module provides utilities for creating simulated streams for testing.

use futures::Stream;
use std::time::Duration;
use tokio::time::sleep;

/// Simulate a readable stream from a vector.
///
/// # Arguments
///
/// * `items` - The items to stream.
/// * `delay` - Optional delay between items.
///
/// # Returns
///
/// A stream that yields the items.
pub fn simulate_readable_stream<T>(
    items: Vec<T>,
    delay: Option<Duration>,
) -> impl Stream<Item = T> + Send
where
    T: Send + 'static,
{
    let delay = delay.unwrap_or(Duration::ZERO);
    futures::stream::unfold(
        (items.into_iter(), delay),
        move |(mut iter, delay)| async move {
            if delay > Duration::ZERO {
                sleep(delay).await;
            }
            iter.next().map(|item| (item, (iter, delay)))
        },
    )
}

/// Simulate a stream that yields values at intervals.
///
/// # Arguments
///
/// * `count` - The number of items to generate.
/// * `interval` - The interval between items.
/// * `generator` - A function to generate each item.
///
/// # Returns
///
/// A stream that yields generated items.
pub fn simulate_interval_stream<T, F>(
    count: usize,
    interval: Duration,
    generator: F,
) -> impl Stream<Item = T> + Send
where
    T: Send + 'static,
    F: Fn(usize) -> T + Send + Sync + 'static,
{
    futures::stream::unfold(
        (0usize, count, interval, generator),
        move |(i, count, interval, gen_fn)| async move {
            if i < count {
                sleep(interval).await;
                Some((gen_fn(i), (i + 1, count, interval, gen_fn)))
            } else {
                None
            }
        },
    )
}

/// Simulate a stream that can be paused and resumed.
pub struct SimulatedStream<T> {
    items: Vec<T>,
    position: usize,
    paused: bool,
}

impl<T> SimulatedStream<T> {
    /// Create a new simulated stream.
    pub fn new(items: Vec<T>) -> Self {
        Self {
            items,
            position: 0,
            paused: false,
        }
    }

    /// Pause the stream.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Resume the stream.
    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// Check if the stream is paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Get the current position.
    pub fn position(&self) -> usize {
        self.position
    }

    /// Get the total number of items.
    pub fn total(&self) -> usize {
        self.items.len()
    }

    /// Check if the stream is finished.
    pub fn is_finished(&self) -> bool {
        self.position >= self.items.len()
    }
}

impl<T: Clone> SimulatedStream<T> {
    /// Get the next item if available.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<T> {
        if self.paused || self.is_finished() {
            return None;
        }
        let item = self.items.get(self.position).cloned();
        self.position += 1;
        item
    }

    /// Collect all remaining items.
    pub fn collect_remaining(&mut self) -> Vec<T> {
        let mut result = Vec::new();
        while let Some(item) = self.next() {
            result.push(item);
        }
        result
    }
}
