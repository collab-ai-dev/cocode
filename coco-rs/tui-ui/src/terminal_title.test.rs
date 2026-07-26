use pretty_assertions::assert_eq;

use super::MAX_TITLE_CHARS;
use super::sanitize_title;

#[test]
fn plain_text_survives_unchanged() {
    assert_eq!(sanitize_title("coco · my-project"), "coco · my-project");
}

#[test]
fn escape_terminators_cannot_break_out_of_the_sequence() {
    // BEL and ESC both end an OSC string; if either survived, everything after
    // it would be interpreted as terminal commands rather than title text.
    let hostile = "title\x07\x1b]0;pwned\x07";
    let sanitized = sanitize_title(hostile);
    assert!(!sanitized.contains('\x07'));
    assert!(!sanitized.contains('\x1b'));
    assert_eq!(sanitized, "title ]0;pwned");
}

#[test]
fn newlines_and_tabs_collapse_into_single_spaces() {
    assert_eq!(sanitize_title("a\n\nb\tc  d"), "a b c d");
}

#[test]
fn bidi_overrides_are_stripped() {
    // RLO makes the remainder render right-to-left, so a title can read as a
    // completely different string than the one it contains.
    let sanitized = sanitize_title("report\u{202e}gnp.exe");
    assert!(!sanitized.contains('\u{202e}'));
    assert_eq!(sanitized, "report gnp.exe");
}

#[test]
fn zero_width_formatting_is_stripped() {
    for invisible in ['\u{200b}', '\u{200d}', '\u{2060}', '\u{feff}'] {
        let sanitized = sanitize_title(&format!("a{invisible}b"));
        assert_eq!(sanitized, "a b", "{invisible:?} must not survive");
    }
}

#[test]
fn c1_controls_are_stripped() {
    // 0x9b is CSI in the C1 block — a one-byte introducer on terminals that
    // decode 8-bit controls.
    let sanitized = sanitize_title("a\u{9b}31mb");
    assert!(!sanitized.contains('\u{9b}'));
}

#[test]
fn long_titles_are_capped_by_chars_not_bytes() {
    // A byte-based cut would land mid-codepoint on multibyte text.
    let long = "字".repeat(MAX_TITLE_CHARS * 2);
    let sanitized = sanitize_title(&long);
    assert_eq!(sanitized.chars().count(), MAX_TITLE_CHARS);
}

#[test]
fn all_hostile_input_sanitizes_to_empty() {
    // The caller distinguishes this from a real title and clears instead of
    // writing an empty one.
    assert_eq!(sanitize_title("\u{200b}\u{202e}\x07"), "");
    assert_eq!(sanitize_title("   "), "");
}
