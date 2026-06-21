use super::*;
use crate::events::LogLevel;

#[test]
fn test_event_log_push() {
    let mut state = EventLogState::new();
    assert!(state.is_empty());

    state.push(RetrievalEvent::WatchStarted {
        workspace: "test".to_string(),
        paths: vec![],
    });

    assert_eq!(state.len(), 1);
    assert!(!state.is_empty());
}

#[test]
fn test_event_log_max_capacity() {
    let mut state = EventLogState::new();

    for i in 0..(MAX_EVENTS + 10) {
        state.push(RetrievalEvent::DiagnosticLog {
            level: LogLevel::Info,
            module: "test".to_string(),
            message: format!("event {}", i),
            fields: Default::default(),
        });
    }

    assert_eq!(state.len(), MAX_EVENTS);
}

#[test]
fn test_event_log_scroll() {
    let mut state = EventLogState::new();

    for i in 0..20 {
        state.push(RetrievalEvent::DiagnosticLog {
            level: LogLevel::Info,
            module: "test".to_string(),
            message: format!("event {}", i),
            fields: Default::default(),
        });
    }

    assert_eq!(state.scroll_offset, 0);
    assert!(state.auto_scroll);

    state.scroll_up(5);
    assert_eq!(state.scroll_offset, 5);
    assert!(!state.auto_scroll);

    state.scroll_down(3);
    assert_eq!(state.scroll_offset, 2);

    state.scroll_to_bottom();
    assert_eq!(state.scroll_offset, 0);
    assert!(state.auto_scroll);
}

#[test]
fn test_format_timestamp() {
    // 10:30:45
    let ts = 10 * 3600 + 30 * 60 + 45;
    assert_eq!(EventLog::format_timestamp(ts), "10:30:45");
}
