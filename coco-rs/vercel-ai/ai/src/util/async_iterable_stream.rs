//! Async iterable stream utilities.
//!
//! This module provides utilities for converting async iterables to streams.

use futures::Stream;
use futures::StreamExt;

/// Convert a Vec into a stream.
///
/// # Arguments
///
/// * `items` - The items to stream.
///
/// # Returns
///
/// A stream that yields the items.
pub fn stream_from_vec<T>(items: Vec<T>) -> impl Stream<Item = T> + Send
where
    T: Send + 'static,
{
    futures::stream::iter(items)
}

/// Create a stream from a range.
///
/// # Arguments
///
/// * `start` - The start of the range.
/// * `end` - The end of the range (exclusive).
///
/// # Returns
///
/// A stream that yields numbers from the range.
pub fn stream_from_range(start: usize, end: usize) -> impl Stream<Item = usize> + Send {
    futures::stream::iter(start..end)
}

/// Create a stream that repeats a value.
///
/// # Arguments
///
/// * `value` - The value to repeat.
/// * `count` - The number of times to repeat.
///
/// # Returns
///
/// A stream that yields the value `count` times.
pub fn stream_repeat<T>(value: T, count: usize) -> impl Stream<Item = T> + Send
where
    T: Clone + Send + 'static,
{
    futures::stream::repeat(value).take(count)
}

/// Create an empty stream.
///
/// # Returns
///
/// An empty stream.
pub fn empty_stream<T>() -> impl Stream<Item = T> + Send
where
    T: Send + 'static,
{
    futures::stream::empty()
}

/// Create a stream with a single value.
///
/// # Arguments
///
/// * `value` - The value to yield.
///
/// # Returns
///
/// A stream that yields the value once.
pub fn once_stream<T>(value: T) -> impl Stream<Item = T> + Send
where
    T: Send + 'static,
{
    futures::stream::once(async move { value })
}

/// Chain two streams together.
///
/// # Arguments
///
/// * `first` - The first stream.
/// * `second` - The second stream.
///
/// # Returns
///
/// A stream that yields items from both streams.
pub fn chain_streams<S1, S2>(first: S1, second: S2) -> impl Stream<Item = S1::Item> + Send
where
    S1: Stream + Send + 'static,
    S2: Stream<Item = S1::Item> + Send + 'static,
{
    first.chain(second)
}
