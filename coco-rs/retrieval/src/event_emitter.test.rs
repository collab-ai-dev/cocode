use super::*;
use crate::events::SearchMode;
use std::sync::Mutex;

// Serialize tests that interact with the global emitter to prevent race conditions
static TEST_MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn test_emit_and_subscribe() {
    let _guard = TEST_MUTEX.lock().unwrap();
    EventEmitter::set_enabled(true);

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
