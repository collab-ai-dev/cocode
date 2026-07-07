use super::*;
use crate::events::RetrievalAggregateEvent;
use crate::events::RetrievalAggregatePhase;
use crate::events::RetrievalAggregateSink;
use crate::events::SearchMode;
use std::sync::Arc;
use std::sync::Mutex;

// Serialize tests that interact with the global emitter to prevent race conditions
static TEST_MUTEX: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct RecordingAggregateSink {
    events: Mutex<Vec<RetrievalAggregateEvent>>,
}

impl RecordingAggregateSink {
    fn events(&self) -> Vec<RetrievalAggregateEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl RetrievalAggregateSink for RecordingAggregateSink {
    fn on_aggregate_event(&self, event: &RetrievalAggregateEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

#[test]
fn test_emit_and_subscribe() {
    let _guard = TEST_MUTEX.lock().unwrap();
    EventEmitter::set_enabled(true);
    EventEmitter::set_aggregate_sink(None);

    let mut rx = EventEmitter::subscribe();

    let event = RetrievalEvent::SearchStarted {
        query_id: "q-1".to_string(),
        query: "test".to_string(),
        mode: SearchMode::Hybrid,
        limit: 10,
    };

    let count = EventEmitter::emit(event.clone());
    assert!(count >= 1);

    // Receive the event
    let received = rx.try_recv().unwrap();
    assert_eq!(received.event_type(), "search_started");
}

#[test]
fn test_enable_disable() {
    let _guard = TEST_MUTEX.lock().unwrap();
    EventEmitter::set_enabled(true); // Ensure clean state
    EventEmitter::set_aggregate_sink(None);

    // Ensure enabled by default
    assert!(EventEmitter::is_enabled());

    // Disable
    EventEmitter::set_enabled(false);
    assert!(!EventEmitter::is_enabled());

    // Emit should be no-op when disabled
    let event = RetrievalEvent::SessionEnded {
        session_id: "s-1".to_string(),
        duration_ms: 100,
    };
    let count = EventEmitter::emit(event);
    assert_eq!(count, 0);

    // Re-enable (restore state for other tests)
    EventEmitter::set_enabled(true);
    assert!(EventEmitter::is_enabled());
}

#[test]
fn test_subscriber_count() {
    let _guard = TEST_MUTEX.lock().unwrap();
    EventEmitter::set_aggregate_sink(None);

    let initial = EventEmitter::subscriber_count();

    let _rx1 = EventEmitter::subscribe();
    assert_eq!(EventEmitter::subscriber_count(), initial + 1);

    let _rx2 = EventEmitter::subscribe();
    assert_eq!(EventEmitter::subscriber_count(), initial + 2);

    drop(_rx1);
    // Note: receiver_count may not immediately reflect dropped receivers
}

#[test]
fn test_scoped_collector() {
    let _guard = TEST_MUTEX.lock().unwrap();
    EventEmitter::set_enabled(true);
    EventEmitter::set_aggregate_sink(None);

    let collector = ScopedEventCollector::new();

    // Emit some events
    emit(RetrievalEvent::SearchStarted {
        query_id: "q-1".to_string(),
        query: "test1".to_string(),
        mode: SearchMode::Bm25,
        limit: 5,
    });

    emit(RetrievalEvent::SearchCompleted {
        query_id: "q-1".to_string(),
        results: vec![],
        total_duration_ms: 50,
        filter: None,
    });

    // Check collected events
    let events = collector.events();
    assert!(events.len() >= 2);
    assert!(collector.has_event(|e| matches!(e, RetrievalEvent::SearchStarted { .. })));
    assert!(collector.has_event(|e| matches!(e, RetrievalEvent::SearchCompleted { .. })));
}

#[test]
fn test_events_of_type() {
    let _guard = TEST_MUTEX.lock().unwrap();
    EventEmitter::set_enabled(true);
    EventEmitter::set_aggregate_sink(None);

    let collector = ScopedEventCollector::new();

    emit(RetrievalEvent::SearchStarted {
        query_id: "q-1".to_string(),
        query: "test".to_string(),
        mode: SearchMode::Vector,
        limit: 10,
    });

    emit(RetrievalEvent::SessionEnded {
        session_id: "s-1".to_string(),
        duration_ms: 100,
    });

    let search_events = collector.events_of_type("search_started");
    assert!(!search_events.is_empty());

    let session_events = collector.events_of_type("session_ended");
    assert!(!session_events.is_empty());
}

#[test]
fn test_aggregate_sink_receives_only_coarse_events() {
    let _guard = TEST_MUTEX.lock().unwrap();
    EventEmitter::set_enabled(true);

    let sink = Arc::new(RecordingAggregateSink::default());
    EventEmitter::set_aggregate_sink(Some(sink.clone()));

    emit(RetrievalEvent::SearchStarted {
        query_id: "q-1".to_string(),
        query: "test".to_string(),
        mode: SearchMode::Hybrid,
        limit: 10,
    });
    emit(RetrievalEvent::QueryRewritten {
        query_id: "q-1".to_string(),
        original: "test".to_string(),
        rewritten: "test expanded".to_string(),
        expansions: vec!["expanded".to_string()],
        translated: false,
        duration_ms: 2,
    });
    emit(RetrievalEvent::SearchError {
        query_id: "q-1".to_string(),
        error: "not ready".to_string(),
        retryable: true,
    });

    let events = sink.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].operation, "search");
    assert_eq!(events[0].phase, RetrievalAggregatePhase::Started);
    assert_eq!(events[1].phase, RetrievalAggregatePhase::Error);
    assert_eq!(events[1].retryable, Some(true));

    EventEmitter::set_aggregate_sink(None);
    emit(RetrievalEvent::SearchCompleted {
        query_id: "q-1".to_string(),
        results: vec![],
        total_duration_ms: 5,
        filter: None,
    });
    assert_eq!(
        sink.events().len(),
        2,
        "clearing the aggregate sink must restore zero aggregate behavior"
    );
}

#[test]
fn test_aggregate_sink_does_not_replace_full_event_subscription() {
    let _guard = TEST_MUTEX.lock().unwrap();
    EventEmitter::set_enabled(true);

    let sink = Arc::new(RecordingAggregateSink::default());
    EventEmitter::set_aggregate_sink(Some(sink.clone()));
    let mut rx = EventEmitter::subscribe();

    emit(RetrievalEvent::SearchStarted {
        query_id: "q-2".to_string(),
        query: "needle".to_string(),
        mode: SearchMode::Bm25,
        limit: 5,
    });

    let detailed = rx.try_recv().unwrap();
    assert!(matches!(detailed, RetrievalEvent::SearchStarted { .. }));
    assert_eq!(sink.events().len(), 1);

    EventEmitter::set_aggregate_sink(None);
}
