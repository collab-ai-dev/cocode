//! Live, in-process view of a sub-agent's in-flight message history.
//!
//! The coordinator threads a [`LiveTranscript`] handle into the spawn's
//! [`crate::AgentQueryConfig::live_transcript`]; the child engine publishes a
//! post-turn snapshot into it and the periodic AgentSummary timer reads it.

use std::sync::Arc;
use std::sync::Mutex;

use coco_messages::Message;

/// Shared snapshot of a sub-agent's message history.
///
/// **Single writer, single reader, instance-owned.** The child
/// [`crate::AgentQueryEngine`] is the sole writer — it calls [`Self::set`]
/// with the full history after every turn finalize. The AgentSummary timer is
/// the sole reader — it calls [`Self::snapshot`] each tick. One handle is
/// minted per spawn and never shared globally, so this is not a global mutable
/// singleton.
///
/// The snapshot is a `Vec<Arc<Message>>`: cloning it is a row of pointer
/// bumps, and a full-snapshot *replace* (rather than incremental append) keeps
/// the reader's view internally consistent — it never observes a half-written
/// turn.
#[derive(Clone, Default)]
pub struct LiveTranscript {
    inner: Arc<Mutex<Vec<Arc<Message>>>>,
}

impl LiveTranscript {
    /// Mint an empty handle. One per spawn that drives a summary timer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Writer (engine): replace the snapshot with the post-turn history.
    ///
    /// Recovers a poisoned lock in place rather than panicking — the only
    /// work done under the guard is a move/clone of `Vec<Arc<Message>>`, which
    /// cannot itself panic, so a poisoned guard can only come from an
    /// unrelated panic elsewhere and the data remains valid.
    pub fn set(&self, messages: Vec<Arc<Message>>) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = messages;
    }

    /// Reader (summary timer): clone the current snapshot.
    pub fn snapshot(&self) -> Vec<Arc<Message>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[cfg(test)]
#[path = "live_transcript.test.rs"]
mod tests;
