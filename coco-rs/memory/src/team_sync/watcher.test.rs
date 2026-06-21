use super::*;

#[test]
fn test_parse_http_status_extracts_code() {
    assert_eq!(parse_http_status("http 413: too many entries"), Some(413));
    assert_eq!(parse_http_status("http 401: unauthorized"), Some(401));
    assert_eq!(parse_http_status("http 200: ok"), Some(200));
}

#[test]
fn test_parse_http_status_returns_none_for_other_shapes() {
    assert_eq!(parse_http_status("network: connection refused"), None);
    assert_eq!(parse_http_status("invalid response: parse error"), None);
    assert_eq!(parse_http_status(""), None);
}

#[test]
fn test_debounce_constant_matches_ts() {
    // Lock the value here so a future tweak to the constant forces a
    // code review.
    assert_eq!(DEBOUNCE_MS, 2_000);
}
