use super::*;

#[test]
fn accepts_auto_and_iso_codes() {
    assert!(is_valid_language("auto"));
    assert!(is_valid_language("en"));
    assert!(is_valid_language("zh"));
    assert!(is_valid_language("pt-br"));
    assert!(is_valid_language("zh-hant"));
}

#[test]
fn rejects_malformed_codes() {
    assert!(!is_valid_language(""));
    assert!(!is_valid_language("english"));
    assert!(!is_valid_language("e"));
    assert!(!is_valid_language("123"));
    assert!(!is_valid_language("en-"));
}

#[test]
fn no_arg_lists_current_and_hint() {
    let out = run("").expect("ok");
    assert!(out.contains("Dictation language:"));
    assert!(out.contains("auto"));
}

#[test]
fn bad_code_is_rejected_without_persisting() {
    let out = run("klingon").expect("ok");
    assert!(out.contains("Unsupported language code"));
}
