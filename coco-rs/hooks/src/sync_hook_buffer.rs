//! Thread-safe FIFO buffer of completed sync hook events for the
//! per-turn reminder pipeline.
//!
//! Hook events from `SessionStart` / `UserPromptSubmit` are held here
//! instead of being emitted directly into the conversation. The reminder
//! pipeline drains the buffer via `HookEventsSource::drain` so the
//! standard generators (`hook_success` / `hook_blocking_error` /
//! `hook_additional_context` / `hook_stopped_continuation`) render them.
//!
//! Drain-on-read semantics: events are consumed on the first drain call
//! and never re-emitted.

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::Mutex;

use coco_system_reminder::HookEvent;

/// Cheap-to-clone handle around a shared FIFO. All clones share the same
/// underlying queue.
#[derive(Debug, Default, Clone)]
pub struct SyncHookEventBuffer {
    inner: Arc<Mutex<VecDeque<HookEvent>>>,
}

impl SyncHookEventBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a hook event to the buffer.
    pub async fn push(&self, ev: HookEvent) {
        self.inner.lock().await.push_back(ev);
    }

    /// Append several hook events in order.
    pub async fn extend<I: IntoIterator<Item = HookEvent>>(&self, evs: I) {
        let mut g = self.inner.lock().await;
        for ev in evs {
            g.push_back(ev);
        }
    }

    /// Drain every buffered event, returning them in FIFO order.
    /// Subsequent calls return an empty `Vec` until new events are pushed.
    pub async fn drain(&self) -> Vec<HookEvent> {
        let mut g = self.inner.lock().await;
        g.drain(..).collect()
    }

    /// `true` iff no events are currently buffered. Mostly for tests.
    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }
}

#[cfg(test)]
#[path = "sync_hook_buffer.test.rs"]
mod tests;
