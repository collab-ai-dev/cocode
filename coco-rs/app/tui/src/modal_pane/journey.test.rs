use super::*;
use crossterm::event::KeyCode;
use crossterm::event::KeyModifiers;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn test_map_key_nav() {
    assert!(matches!(
        map_key(key(KeyCode::Char('j'))),
        Some(TuiCommand::CursorDown)
    ));
    assert!(matches!(
        map_key(key(KeyCode::Down)),
        Some(TuiCommand::CursorDown)
    ));
    assert!(matches!(
        map_key(key(KeyCode::Char('k'))),
        Some(TuiCommand::CursorUp)
    ));
    assert!(matches!(
        map_key(key(KeyCode::Up)),
        Some(TuiCommand::CursorUp)
    ));
}

#[test]
fn test_map_key_enter_esc() {
    assert!(matches!(
        map_key(key(KeyCode::Enter)),
        Some(TuiCommand::SubmitInput)
    ));
    assert!(matches!(
        map_key(key(KeyCode::Esc)),
        Some(TuiCommand::Cancel)
    ));
}

#[test]
fn test_map_key_action_chars_route_as_insertchar() {
    // e/d/y/n reach the interceptor as InsertChar (not eaten as filter input).
    for c in ['e', 'd', 'y', 'n'] {
        assert!(
            matches!(map_key(key(KeyCode::Char(c))), Some(TuiCommand::InsertChar(x)) if x == c),
            "char {c} should map to InsertChar"
        );
    }
}

#[test]
fn test_map_key_unhandled_returns_none() {
    assert!(map_key(key(KeyCode::Char('z'))).is_none());
    assert!(map_key(key(KeyCode::Tab)).is_none());
}
