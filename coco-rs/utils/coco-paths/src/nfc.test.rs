use super::*;
use pretty_assertions::assert_eq;

#[test]
fn normalize_ascii_is_identity() {
    assert_eq!(normalize_nfc("hello"), "hello");
}

#[test]
fn normalize_collapses_decomposed_to_precomposed() {
    // 'é' written as 'e' + U+0301 (combining acute) should fold to
    // the precomposed U+00E9.
    let decomposed = "e\u{0301}";
    let precomposed = "\u{00E9}";
    assert_eq!(normalize_nfc(decomposed), precomposed);
}

#[test]
fn normalize_idempotent() {
    let once = normalize_nfc("e\u{0301}");
    let twice = normalize_nfc(&once);
    assert_eq!(once, twice);
}
