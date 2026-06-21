//! Stream consumption utility.
//!
//! Reads a stream to completion, discarding all output.

use futures::StreamExt;

/// Consume a stream to completion, discarding all items.
///
/// This is useful when you need a stream's side effects (e.g., callbacks)
/// but don't need to process the items.
pub async fn consume_stream<S, T>(mut stream: S)
where
    S: futures::Stream<Item = T> + Unpin,
{
    while stream.next().await.is_some() {}
}

#[cfg(test)]
#[path = "consume_stream.test.rs"]
mod tests;
