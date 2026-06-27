use super::*;
use pretty_assertions::assert_eq;

#[test]
fn djb2_seed_value_for_empty_string() {
    // djb2 starts at 5381; no characters to fold → seed value.
    assert_eq!(djb2(""), 5381);
}

#[test]
fn djb2_single_ascii_char() {
    // 5381 * 33 + 97 = 177670
    assert_eq!(djb2("a"), 177670);
}

#[test]
fn djb2_multichar_with_i32_wrap() {
    // Hand-computed: walks through "hello" with the wrap-at-i32
    // semantics that JS `| 0` provides. The intermediate value after
    // the second 'l' exceeds 2^31 and wraps.
    assert_eq!(djb2("hello"), 261238937);
}

#[test]
fn simple_hash_seed_in_base36() {
    // 5381 in base36 = "45h".
    assert_eq!(simple_hash(""), "45h");
}

#[test]
fn simple_hash_single_char() {
    // 177670 in base36 = "3t3a".
    assert_eq!(simple_hash("a"), "3t3a");
}

#[test]
fn simple_hash_multichar_with_wrap() {
    // 261238937 in base36 = "4bj995".
    assert_eq!(simple_hash("hello"), "4bj995");
}
