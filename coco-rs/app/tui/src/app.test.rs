use std::collections::VecDeque;

use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::TuiOnlyEvent;
use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyEventState;
use crossterm::event::KeyModifiers;

use super::DEFERRED_CORE_EVENT_LIMIT;
use super::DeferredCoreEvent;
use super::convert_crossterm_event;
use super::defer_core_event;
use crate::events::TuiEvent;

fn key(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
    KeyEvent {
        code,
        modifiers,
        kind,
        state: KeyEventState::NONE,
    }
}

fn converts_to_key_event(key_event: KeyEvent) -> bool {
    matches!(
        convert_crossterm_event(Event::Key(key_event)),
        Some(TuiEvent::Key(_))
    )
}

#[test]
fn crossterm_filter_accepts_key_press() {
    assert!(converts_to_key_event(key(
        KeyCode::Left,
        KeyModifiers::NONE,
        KeyEventKind::Press,
    )));
}

#[test]
fn crossterm_filter_accepts_navigation_repeats() {
    assert!(converts_to_key_event(key(
        KeyCode::Left,
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    )));
    assert!(converts_to_key_event(key(
        KeyCode::Right,
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    )));
}

#[test]
fn crossterm_filter_rejects_key_release() {
    assert!(
        convert_crossterm_event(Event::Key(key(
            KeyCode::Left,
            KeyModifiers::NONE,
            KeyEventKind::Release,
        )))
        .is_none()
    );
}

#[test]
fn crossterm_filter_rejects_exit_chord_repeats() {
    assert!(
        convert_crossterm_event(Event::Key(key(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            KeyEventKind::Repeat,
        )))
        .is_none()
    );
    assert!(
        convert_crossterm_event(Event::Key(key(
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
            KeyEventKind::Repeat,
        )))
        .is_none()
    );
}

#[test]
fn crossterm_filter_rejects_one_shot_action_repeats() {
    assert!(
        convert_crossterm_event(Event::Key(key(
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        )))
        .is_none()
    );
    assert!(
        convert_crossterm_event(Event::Key(key(
            KeyCode::Esc,
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        )))
        .is_none()
    );
}

#[test]
fn crossterm_filter_accepts_plain_character_repeat_only() {
    assert!(converts_to_key_event(key(
        KeyCode::Char('a'),
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    )));
    assert!(
        convert_crossterm_event(Event::Key(key(
            KeyCode::Char('f'),
            KeyModifiers::CONTROL,
            KeyEventKind::Repeat,
        )))
        .is_none()
    );
}

fn lossy_text(n: usize) -> CoreEvent {
    CoreEvent::Stream(AgentStreamEvent::TextDelta {
        turn_id: format!("turn-{n}"),
        delta: "x".to_string(),
    })
}

#[test]
fn deferred_event_buffer_coalesces_stream_deltas() {
    let mut buffer = VecDeque::new();

    assert!(matches!(
        defer_core_event(
            &mut buffer,
            CoreEvent::Stream(AgentStreamEvent::TextDelta {
                turn_id: "t1".to_string(),
                delta: "hello ".to_string(),
            }),
        ),
        DeferredCoreEvent::Buffered
    ));
    assert!(matches!(
        defer_core_event(
            &mut buffer,
            CoreEvent::Stream(AgentStreamEvent::TextDelta {
                turn_id: "t1".to_string(),
                delta: "world".to_string(),
            }),
        ),
        DeferredCoreEvent::Buffered
    ));

    assert_eq!(buffer.len(), 1);
    let CoreEvent::Stream(AgentStreamEvent::TextDelta { delta, .. }) = &buffer[0] else {
        panic!("expected coalesced text delta");
    };
    assert_eq!(delta, "hello world");
}

#[test]
fn deferred_event_buffer_drops_lossy_overflow() {
    let mut buffer = VecDeque::new();
    for n in 0..DEFERRED_CORE_EVENT_LIMIT {
        assert!(matches!(
            defer_core_event(&mut buffer, lossy_text(n)),
            DeferredCoreEvent::Buffered
        ));
    }

    assert!(matches!(
        defer_core_event(&mut buffer, lossy_text(DEFERRED_CORE_EVENT_LIMIT)),
        DeferredCoreEvent::Dropped
    ));
    assert_eq!(buffer.len(), DEFERRED_CORE_EVENT_LIMIT);
}

#[test]
fn deferred_event_buffer_preserves_terminal_events_at_capacity() {
    let mut buffer = VecDeque::new();
    for n in 0..DEFERRED_CORE_EVENT_LIMIT {
        assert!(matches!(
            defer_core_event(&mut buffer, lossy_text(n)),
            DeferredCoreEvent::Buffered
        ));
    }

    let terminal = CoreEvent::Tui(TuiOnlyEvent::PromptEditorCompleted {
        content: "done".to_string(),
        modified: true,
    });
    assert!(matches!(
        defer_core_event(&mut buffer, terminal),
        DeferredCoreEvent::Buffered
    ));
    assert_eq!(buffer.len(), DEFERRED_CORE_EVENT_LIMIT);
    assert!(buffer.iter().any(|event| matches!(
        event,
        CoreEvent::Tui(TuiOnlyEvent::PromptEditorCompleted { .. })
    )));
}

#[test]
fn deferred_event_buffer_processes_non_lossy_when_no_lossy_slot_exists() {
    let mut buffer = VecDeque::new();
    for n in 0..DEFERRED_CORE_EVENT_LIMIT {
        buffer.push_back(CoreEvent::Protocol(ServerNotification::KeepAlive {
            timestamp: n as i64,
        }));
    }

    let event = CoreEvent::Protocol(ServerNotification::KeepAlive { timestamp: 999 });
    let DeferredCoreEvent::ProcessNow(event) = defer_core_event(&mut buffer, event) else {
        panic!("expected oldest non-lossy event to process immediately");
    };
    assert!(matches!(
        *event,
        CoreEvent::Protocol(ServerNotification::KeepAlive { timestamp: 0 })
    ));
    assert_eq!(buffer.len(), DEFERRED_CORE_EVENT_LIMIT);
    assert!(matches!(
        buffer.back(),
        Some(CoreEvent::Protocol(ServerNotification::KeepAlive {
            timestamp: 999,
        }))
    ));
}
