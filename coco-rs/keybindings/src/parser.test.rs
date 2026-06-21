use super::KeyChord;
use super::KeyCombo;
use super::ParseError;
use super::parse_chord;
use super::parse_combo;

fn combo(key: &str) -> KeyCombo {
    KeyCombo {
        ctrl: false,
        shift: false,
        alt: false,
        meta: false,
        super_key: false,
        key: key.into(),
    }
}

#[test]
fn parses_plain_key() {
    assert_eq!(parse_combo("a").unwrap(), combo("a"));
    assert_eq!(parse_combo("enter").unwrap(), combo("enter"));
    assert_eq!(parse_combo("f1").unwrap(), combo("f1"));
}

#[test]
fn parses_ctrl_modifier() {
    let got = parse_combo("ctrl+a").unwrap();
    assert!(got.ctrl);
    assert!(!got.shift);
    assert_eq!(got.key, "a");
}

#[test]
fn parses_multi_modifier_case_insensitive() {
    let got = parse_combo("Ctrl+Shift+P").unwrap();
    assert!(got.ctrl && got.shift && !got.alt && !got.meta);
    assert_eq!(got.key, "p");
}

#[test]
fn parses_meta_distinct_from_super() {
    // `meta` (alt-equivalent in terminals) and `super`
    // (cmd/win, kitty-keyboard-protocol only) are kept separate.
    let m = parse_combo("meta+k").unwrap();
    assert!(m.meta && !m.super_key);

    for token in ["cmd", "command", "super", "win"] {
        let got = parse_combo(&format!("{token}+k")).unwrap();
        assert!(got.super_key, "token `{token}` should set super");
        assert!(!got.meta, "token `{token}` should NOT set meta");
        assert_eq!(got.key, "k");
    }
}

#[test]
fn parses_alt_aliases() {
    for token in ["alt", "opt", "option"] {
        let got = parse_combo(&format!("{token}+k")).unwrap();
        assert!(got.alt, "token `{token}` should set alt");
        assert_eq!(got.key, "k");
    }
}

#[test]
fn normalizes_named_keys() {
    // Canonical names: `escape`, `enter`, `delete`, `backspace`,
    // `pageup`, `pagedown`, `space`.
    assert_eq!(parse_combo("Return").unwrap().key, "enter");
    assert_eq!(parse_combo("Escape").unwrap().key, "escape");
    assert_eq!(parse_combo("esc").unwrap().key, "escape");
    assert_eq!(parse_combo("delete").unwrap().key, "delete");
    assert_eq!(parse_combo("del").unwrap().key, "delete");
    assert_eq!(parse_combo("backspace").unwrap().key, "backspace");
    assert_eq!(parse_combo("bs").unwrap().key, "backspace");
    assert_eq!(parse_combo("pgup").unwrap().key, "pageup");
    assert_eq!(parse_combo("pgdn").unwrap().key, "pagedown");
    assert_eq!(parse_combo("space").unwrap().key, "space");
}

#[test]
fn rejects_empty_and_multi_key() {
    assert!(matches!(parse_combo(""), Err(ParseError::EmptyCombo)));
    assert!(matches!(
        parse_combo("ctrl"),
        Err(ParseError::MissingBaseKey { .. }),
    ));
    assert!(matches!(
        parse_combo("a+b"),
        Err(ParseError::MultipleBaseKeys { .. }),
    ));
    assert!(matches!(parse_chord(""), Err(ParseError::EmptyChord)));
}

#[test]
fn parses_chord_with_two_combos_whitespace_separated() {
    // Chord syntax: whitespace-separated steps.
    let chord = parse_chord("ctrl+x ctrl+k").unwrap();
    assert!(!chord.is_single());
    assert_eq!(chord.0.len(), 2);
    assert!(chord.0[0].ctrl && chord.0[0].key == "x");
    assert!(chord.0[1].ctrl && chord.0[1].key == "k");
}

#[test]
fn handles_lone_space_as_space_key() {
    // A literal single-space input is the space-key binding, not an empty chord.
    let chord = parse_chord(" ").unwrap();
    assert!(chord.is_single());
    assert_eq!(chord.0[0].key, "space");
}

#[test]
fn collapses_extra_whitespace_between_combos() {
    let chord = parse_chord("ctrl+x   ctrl+k").unwrap();
    assert_eq!(chord.0.len(), 2);
}

#[test]
fn single_key_is_single_chord() {
    let chord = parse_chord("enter").unwrap();
    assert!(chord.is_single());
    assert_eq!(chord, KeyChord(vec![combo("enter")]));
}

#[test]
fn parse_error_implements_display() {
    let err = parse_combo("ctrl").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("ctrl"));
    assert!(msg.contains("base key"));
}
