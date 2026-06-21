use super::*;

#[test]
fn parse_pasted_full_redirect_url() {
    let p = parse_pasted_callback("http://localhost:1455/auth/callback?code=abc&state=xyz");
    assert_eq!(p.code.as_deref(), Some("abc"));
    assert_eq!(p.state.as_deref(), Some("xyz"));
    assert!(p.error.is_none());
}

#[test]
fn parse_pasted_bare_query() {
    let p = parse_pasted_callback("code=abc&state=xyz");
    assert_eq!(p.code.as_deref(), Some("abc"));
    assert_eq!(p.state.as_deref(), Some("xyz"));
}

#[test]
fn parse_pasted_bare_code() {
    let p = parse_pasted_callback("just-a-code-token");
    assert_eq!(p.code.as_deref(), Some("just-a-code-token"));
    assert!(p.state.is_none());
}

#[test]
fn parse_pasted_error_redirect() {
    let p = parse_pasted_callback("http://localhost/auth/callback?error=access_denied");
    assert_eq!(p.error.as_deref(), Some("access_denied"));
    assert!(p.code.is_none());
}
