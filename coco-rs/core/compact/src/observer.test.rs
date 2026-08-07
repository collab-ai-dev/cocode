use std::sync::Arc;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;

use coco_messages::Message;
use coco_types::CompactTrigger;

use super::*;

/// Test observer that counts invocations.
struct CountingObserver {
    complete_count: AtomicI32,
    post_compact_count: AtomicI32,
}

impl CountingObserver {
    fn new() -> Self {
        Self {
            complete_count: AtomicI32::new(0),
            post_compact_count: AtomicI32::new(0),
        }
    }
}

#[async_trait::async_trait]
impl CompactionObserver for CountingObserver {
    async fn on_compaction_complete(
        &self,
        _result: &CompactResult,
        _is_main_agent: bool,
    ) -> Result<(), BoxedError> {
        self.complete_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn on_post_compact(&self, _compacted_messages: &[Message]) -> Result<(), BoxedError> {
        self.post_compact_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Observer that only implements on_compaction_complete (uses default on_post_compact).
struct LegacyObserver {
    called: AtomicI32,
}

impl LegacyObserver {
    fn new() -> Self {
        Self {
            called: AtomicI32::new(0),
        }
    }
}

#[async_trait::async_trait]
impl CompactionObserver for LegacyObserver {
    async fn on_compaction_complete(
        &self,
        _result: &CompactResult,
        _is_main_agent: bool,
    ) -> Result<(), BoxedError> {
        self.called.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn dummy_result() -> CompactResult {
    let boundary = Message::System(coco_messages::SystemMessage::CompactBoundary(
        coco_messages::SystemCompactBoundaryMessage {
            uuid: uuid::Uuid::new_v4(),
            tokens_before: 100,
            tokens_after: 50,
            trigger: CompactTrigger::Auto,
            user_context: None,
            messages_summarized: None,
            pre_compact_discovered_tools: vec![],
            preserved_segment: None,
        },
    ));
    CompactResult {
        boundary_marker: boundary,
        raw_summary: None,
        summary_messages: vec![],
        attachments: vec![],
        messages_to_keep: vec![],
        hook_results: vec![],
        user_display_message: None,
        pre_compact_tokens: 100,
        post_compact_tokens: 50,
        true_post_compact_tokens: 50,
        is_recompaction: false,
        trigger: CompactTrigger::Auto,
    }
}

#[tokio::test]
async fn test_notify_all_calls_all_observers() {
    let obs1 = Arc::new(CountingObserver::new());
    let obs2 = Arc::new(CountingObserver::new());

    let mut registry = CompactionObserverRegistry::new();
    registry.register(obs1.clone());
    registry.register(obs2.clone());

    registry
        .notify_all(&dummy_result(), /*is_main_agent*/ true)
        .await;

    assert_eq!(obs1.complete_count.load(Ordering::SeqCst), 1);
    assert_eq!(obs2.complete_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_notify_post_compact_calls_all_observers() {
    let obs = Arc::new(CountingObserver::new());

    let mut registry = CompactionObserverRegistry::new();
    registry.register(obs.clone());

    let messages: Vec<Message> = vec![];
    registry.notify_post_compact(&messages).await;

    assert_eq!(obs.post_compact_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_legacy_observer_default_post_compact() {
    let obs = Arc::new(LegacyObserver::new());

    let mut registry = CompactionObserverRegistry::new();
    registry.register(obs.clone());

    let messages: Vec<Message> = vec![];
    registry.notify_post_compact(&messages).await;

    registry
        .notify_all(&dummy_result(), /*is_main_agent*/ false)
        .await;
    assert_eq!(obs.called.load(Ordering::SeqCst), 1);
}

#[test]
fn test_registry_len_and_is_empty() {
    let mut registry = CompactionObserverRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);

    registry.register(Arc::new(CountingObserver::new()));
    assert!(!registry.is_empty());
    assert_eq!(registry.len(), 1);
}
