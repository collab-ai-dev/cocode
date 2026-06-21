use super::*;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

#[test]
fn test_view_mode_navigation() {
    assert_eq!(ViewMode::Search.next(), ViewMode::Index);
    assert_eq!(ViewMode::Debug.next(), ViewMode::Search);
    assert_eq!(ViewMode::Search.prev(), ViewMode::Debug);
    assert_eq!(ViewMode::Index.prev(), ViewMode::Search);
}

#[test]
fn test_view_mode_index() {
    assert_eq!(ViewMode::Search.index(), 0);
    assert_eq!(ViewMode::Debug.index(), 4);
    assert_eq!(ViewMode::from_index(0), Some(ViewMode::Search));
    assert_eq!(ViewMode::from_index(4), Some(ViewMode::Debug));
    assert_eq!(ViewMode::from_index(5), None);
}

#[test]
fn test_keybindings() {
    let quit_key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    assert!(keybindings::is_quit(&quit_key));

    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(keybindings::is_quit(&ctrl_c));

    let tab_key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    assert!(keybindings::is_tab(&tab_key));

    let shift_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
    assert!(keybindings::is_shift_tab(&shift_tab));
}
