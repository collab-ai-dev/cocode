//! Stitchable stream utilities.
//!
//! This module provides utilities for stitching multiple streams together.

use futures::Stream;
use std::pin::Pin;

/// A stream that can be stitched with another stream.
pub struct StitchableStream<T>
where
    T: Send + 'static,
{
    streams: Vec<Pin<Box<dyn Stream<Item = T> + Send>>>,
}

impl<T> StitchableStream<T>
where
    T: Send + 'static,
{
    /// Create a new empty stitchable stream.
    pub fn new() -> Self {
        Self {
            streams: Vec::new(),
        }
    }

    /// Create from a single stream.
    pub fn from_stream<S>(stream: S) -> Self
    where
        S: Stream<Item = T> + Send + 'static,
    {
        Self {
            streams: vec![Box::pin(stream)],
        }
    }

    /// Stitch another stream to the end.
    pub fn stitch<S>(mut self, stream: S) -> Self
    where
        S: Stream<Item = T> + Send + 'static,
    {
        self.streams.push(Box::pin(stream));
        self
    }

    /// Convert to a single stream.
    pub fn into_stream(self) -> Pin<Box<dyn Stream<Item = T> + Send>> {
        Box::pin(futures::stream::select_all(self.streams))
    }
}

impl<T> Default for StitchableStream<T>
where
    T: Send + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

/// Create a stitchable stream from a single stream.
///
/// # Arguments
///
/// * `stream` - The initial stream.
///
/// # Returns
///
/// A stitchable stream.
pub fn create_stitchable_stream<T, S>(stream: S) -> StitchableStream<T>
where
    S: Stream<Item = T> + Send + 'static,
    T: Send + 'static,
{
    StitchableStream::from_stream(stream)
}

/// Stitch multiple streams together in sequence.
///
/// # Arguments
///
/// * `streams` - The streams to stitch.
///
/// # Returns
///
/// A single stream that yields items from each stream in order.
pub fn stitch_streams<T, S>(streams: Vec<S>) -> impl Stream<Item = T> + Send
where
    S: Stream<Item = T> + Send + 'static,
    T: Send + 'static,
{
    let pinned: Vec<Pin<Box<dyn Stream<Item = T> + Send>>> = streams
        .into_iter()
        .map(|s| Box::pin(s) as Pin<Box<dyn Stream<Item = T> + Send>>)
        .collect();
    futures::stream::select_all(pinned)
}
