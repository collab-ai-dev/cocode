use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_bounded_text_caps_bytes() {
    let text = BoundedText::new("abcdefghij", 4);
    assert_eq!(text.as_str(), "abcd");
}

#[test]
fn test_bounded_text_never_splits_multibyte_char() {
    // Each `中` is 3 bytes; a 4-byte budget must not cut the second char apart.
    let text = BoundedText::new("中中中", 4);
    assert_eq!(text.as_str(), "中");
    assert!(text.as_str().is_char_boundary(text.as_str().len()));
}

#[test]
fn test_bounded_text_trims_trailing_whitespace_from_cut() {
    let text = BoundedText::new("ab   cdef", 5);
    assert_eq!(text.as_str(), "ab");
}

#[test]
fn test_bounded_text_short_and_objective_budgets() {
    let long = "x".repeat(SHORT_TEXT_BUDGET + 100);
    assert_eq!(BoundedText::short(&long).as_str().len(), SHORT_TEXT_BUDGET);
    let longer = "y".repeat(OBJECTIVE_BUDGET + 100);
    assert_eq!(
        BoundedText::objective(&longer).as_str().len(),
        OBJECTIVE_BUDGET
    );
}
