//! Compaction observer pattern for cache invalidation and post-compact hooks.
//!
//! Two callback hooks:
//! - `on_compaction_complete`: receives the `CompactResult` metadata
//! - `on_post_compact`: receives the final compacted messages, allowing
//!   observers to inspect/clean up based on actual message content

use std::sync::Arc;

use coco_error::BoxedError;
use coco_messages::Message;

use crate::types::CompactResult;

/// Observer trait for compaction events.
///
/// Implementors can clear caches, update indices, or perform any cleanup
/// that should happen after compaction. This replaces hardcoded post-compact
/// cleanup lists with an extensible observer pattern.
#[async_trait::async_trait]
pub trait CompactionObserver: Send + Sync {
    /// Called after a compaction completes with result metadata.
    async fn on_compaction_complete(
        &self,
        result: &CompactResult,
        is_main_agent: bool,
    ) -> Result<(), BoxedError>;

    /// Called after compaction with the final compacted message list.
    ///
    /// Default implementation is a no-op so existing observers that only
    /// care about `on_compaction_complete` do not need to implement this.
    async fn on_post_compact(&self, _compacted_messages: &[Message]) -> Result<(), BoxedError> {
        Ok(())
    }
}

/// Registry of compaction observers.
#[derive(Default)]
pub struct CompactionObserverRegistry {
    observers: Vec<Arc<dyn CompactionObserver>>,
}

impl CompactionObserverRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, observer: Arc<dyn CompactionObserver>) {
        self.observers.push(observer);
    }

    /// Notify all observers of a compaction event (result metadata).
    pub async fn notify_all(&self, result: &CompactResult, is_main_agent: bool) {
        for observer in &self.observers {
            if let Err(e) = observer.on_compaction_complete(result, is_main_agent).await {
                tracing::warn!("compaction observer on_compaction_complete error: {e}");
            }
        }
    }

    /// Notify all observers with the final compacted messages.
    ///
    /// Generic over `Borrow<Message>` so the engine can pass its
    /// `&[Arc<Message>]` snapshot without a separate deep-clone bridge.
    /// The trait method itself stays object-safe (`&[Message]`); we
    /// materialize once per call (rare — fires only at compaction).
    pub async fn notify_post_compact<M: std::borrow::Borrow<Message>>(
        &self,
        compacted_messages: &[M],
    ) {
        if self.observers.is_empty() {
            return;
        }
        let owned: Vec<Message> = compacted_messages
            .iter()
            .map(|m| m.borrow().clone())
            .collect();
        for observer in &self.observers {
            if let Err(e) = observer.on_post_compact(&owned).await {
                tracing::warn!("compaction observer on_post_compact error: {e}");
            }
        }
    }

    /// Number of registered observers.
    pub fn len(&self) -> usize {
        self.observers.len()
    }

    /// Whether the registry has no observers.
    pub fn is_empty(&self) -> bool {
        self.observers.is_empty()
    }
}

#[cfg(test)]
#[path = "observer.test.rs"]
mod tests;
