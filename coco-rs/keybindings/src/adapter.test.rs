use super::from_crossterm;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyEventState;
use crossterm::event::KeyModifiers;

fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: mods,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

#[test]
fn maps_char_lowercase() {
    let combo = from_crossterm(key(KeyCode::Char('A'), KeyModifiers::SHIFT)).unwrap();
    assert_eq!(combo.key, "a");
    assert!(combo.shift);
}

#[test]
fn maps_named_keys() {
    assert_eq!(
        from_crossterm(key(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap()
            .key,
        "enter",
    );
    assert_eq!(
        from_crossterm(key(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap()
            .key,
        "escape",
    );
    assert_eq!(
        from_crossterm(key(KeyCode::Up, KeyModifiers::NONE))
            .unwrap()
            .key,
        "up",
    );
    assert_eq!(
        from_crossterm(key(KeyCode::PageUp, KeyModifiers::NONE))
            .unwrap()
            .key,
        "pageup",
    );
}

#[test]
fn maps_function_keys() {
    let combo = from_crossterm(key(KeyCode::F(1), KeyModifiers::NONE)).unwrap();
    assert_eq!(combo.key, "f1");
}

#[test]
fn ctrl_modifier_preserved() {
    let combo = from_crossterm(key(KeyCode::Char('c'), KeyModifiers::CONTROL)).unwrap();
    assert!(combo.ctrl);
    assert_eq!(combo.key, "c");
}

#[test]
fn escape_strips_alt_and_meta_quirk() {
    // Terminals set alt/meta on escape, but a plain `escape` binding should
    // still match. Both modifiers are cleared.
    let combo = from_crossterm(key(KeyCode::Esc, KeyModifiers::ALT | KeyModifiers::META)).unwrap();
    assert!(!combo.alt);
    assert!(!combo.meta);
    assert_eq!(combo.key, "escape");
}

#[test]
fn back_tab_sets_shift() {
    let combo = from_crossterm(key(KeyCode::BackTab, KeyModifiers::NONE)).unwrap();
    assert_eq!(combo.key, "tab");
    assert!(combo.shift);
}

#[test]
fn super_modifier_kept_distinct_from_meta() {
    // crossterm SUPER → KeyCombo::super_key (NOT meta) so macOS
    // `cmd+c` is distinguishable from terminal `meta+c`.
    let combo = from_crossterm(key(KeyCode::Char('k'), KeyModifiers::SUPER)).unwrap();
    assert!(combo.super_key);
    assert!(!combo.meta);
}

#[test]
fn meta_modifier_kept_distinct_from_super() {
    let combo = from_crossterm(key(KeyCode::Char('k'), KeyModifiers::META)).unwrap();
    assert!(combo.meta);
    assert!(!combo.super_key);
}

#[test]
fn null_keycode_returns_none() {
    assert!(from_crossterm(key(KeyCode::Null, KeyModifiers::NONE)).is_none());
}
