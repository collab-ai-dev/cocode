use super::*;

#[test]
fn parses_bare_sentinel() {
    let parsed = parse_sentinel("__SENTINEL__\nstatus", "__SENTINEL__").expect("parsed");
    assert_eq!(parsed.args, "");
    assert_eq!(parsed.status, "status");
}

#[test]
fn parses_args_after_sentinel() {
    let parsed = parse_sentinel("__SENTINEL__ keep auth\nstatus", "__SENTINEL__").expect("parsed");
    assert_eq!(parsed.args, "keep auth");
    assert_eq!(parsed.status, "status");
}

#[test]
fn returns_none_for_missing_sentinel() {
    assert!(parse_sentinel("hello\nworld", "__SENTINEL__").is_none());
}

#[test]
fn handles_missing_status_lines() {
    let parsed = parse_sentinel("__SENTINEL__", "__SENTINEL__").expect("parsed");
    assert_eq!(parsed.args, "");
    assert_eq!(parsed.status, "");
}
