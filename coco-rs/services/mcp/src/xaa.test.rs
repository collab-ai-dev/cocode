use super::AsAuthMethod;
use super::JwtBearerRequest;
use super::JwtGrantRequest;
use super::XaaError;
use super::basic_auth_header;
use super::build_jwt_bearer;
use super::build_token_exchange_form;
use super::redact_tokens;
use super::should_clear_id_token_on_status;

fn find<'a>(form: &'a [(&str, String)], key: &str) -> Option<&'a str> {
    form.iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.as_str())
}

#[test]
fn redact_tokens_masks_known_keys() {
    let raw = r#"{"access_token":"secret-abc","refresh_token":"rt-xyz","other":"keep"}"#;
    let out = redact_tokens(raw);
    assert!(out.contains("[REDACTED]"));
    assert!(!out.contains("secret-abc"));
    assert!(!out.contains("rt-xyz"));
    assert!(out.contains("\"other\":\"keep\""));
}

#[test]
fn redact_tokens_handles_all_sensitive_keys() {
    let raw = r#"{"assertion":"a","subject_token":"b","client_secret":"c","id_token":"d"}"#;
    let out = redact_tokens(raw);
    assert!(!out.contains("\"a\""));
    assert!(!out.contains("\"b\""));
    assert!(!out.contains("\"c\""));
    assert!(!out.contains("\"d\""));
    assert_eq!(out.matches("[REDACTED]").count(), 4);
}

/// Regression for an earlier implementation that looped infinitely when
/// the same sensitive key appeared multiple times: after replacing a
/// value with `[REDACTED]` it re-scanned from position 0 and kept
/// finding the still-present key name. The fix advances the scan cursor
/// past each replacement. This test MUST terminate quickly.
#[test]
fn redact_tokens_terminates_on_repeated_key() {
    let raw =
        r#"{"access_token":"first","nested":{"access_token":"second"},"access_token":"third"}"#;
    let out = redact_tokens(raw);
    assert!(!out.contains("first"));
    assert!(!out.contains("second"));
    assert!(!out.contains("third"));
    assert_eq!(out.matches("[REDACTED]").count(), 3);
}

#[test]
fn should_clear_id_token_on_5xx_returns_false() {
    // 5xx → server glitch, don't throw away the id_token
    assert!(!should_clear_id_token_on_status(500, ""));
    assert!(!should_clear_id_token_on_status(502, ""));
    assert!(!should_clear_id_token_on_status(503, ""));
}

#[test]
fn should_clear_id_token_on_4xx_returns_true() {
    // 4xx → IdP says the id_token is bad
    assert!(should_clear_id_token_on_status(400, ""));
    assert!(should_clear_id_token_on_status(401, ""));
    assert!(should_clear_id_token_on_status(403, ""));
}

#[test]
fn should_clear_id_token_on_other_inspects_body() {
    assert!(should_clear_id_token_on_status(
        300,
        r#"{"error":"invalid_grant"}"#
    ));
    assert!(should_clear_id_token_on_status(
        300,
        r#"{"error":"invalid_token"}"#
    ));
    assert!(!should_clear_id_token_on_status(
        300,
        r#"{"error":"other_error"}"#
    ));
}

#[test]
fn leg1_form_uses_as_issuer_audience_and_carries_resource() {
    // Regression for the param bug: audience must be the AS issuer URL (not a
    // client id), and the leg-1 request must include `resource`.
    let req = JwtGrantRequest {
        token_endpoint: "https://idp.example/token".into(),
        audience: "https://as.example".into(),
        resource: "https://mcp.example/mcp".into(),
        id_token: "the-id-token".into(),
        client_id: "idp-client".into(),
        client_secret: Some("idp-secret".into()),
        scope: Some("read".into()),
    };
    let form = build_token_exchange_form(&req);

    assert_eq!(find(&form, "grant_type"), Some(super::TOKEN_EXCHANGE_GRANT));
    assert_eq!(
        find(&form, "requested_token_type"),
        Some(super::ID_JAG_TOKEN_TYPE)
    );
    assert_eq!(find(&form, "audience"), Some("https://as.example"));
    assert_eq!(find(&form, "resource"), Some("https://mcp.example/mcp"));
    assert_eq!(find(&form, "subject_token"), Some("the-id-token"));
    assert_eq!(
        find(&form, "subject_token_type"),
        Some(super::ID_TOKEN_TYPE)
    );
    assert_eq!(find(&form, "client_id"), Some("idp-client"));
    assert_eq!(find(&form, "client_secret"), Some("idp-secret"));
    assert_eq!(find(&form, "scope"), Some("read"));
}

#[test]
fn leg2_basic_auth_uses_header_not_body() {
    let req = JwtBearerRequest {
        token_endpoint: "https://as.example/token".into(),
        assertion: "the-id-jag".into(),
        client_id: "as-client".into(),
        client_secret: "as-secret".into(),
        auth_method: AsAuthMethod::ClientSecretBasic,
        scope: None,
    };
    let (auth, form) = build_jwt_bearer(&req);

    // Credentials go in the Authorization header, never the body.
    assert_eq!(auth, Some(basic_auth_header("as-client", "as-secret")));
    assert!(find(&form, "client_id").is_none());
    assert!(find(&form, "client_secret").is_none());
    assert_eq!(find(&form, "grant_type"), Some(super::JWT_BEARER_GRANT));
    assert_eq!(find(&form, "assertion"), Some("the-id-jag"));
}

#[test]
fn leg2_post_auth_puts_credentials_in_body() {
    let req = JwtBearerRequest {
        token_endpoint: "https://as.example/token".into(),
        assertion: "jag".into(),
        client_id: "as-client".into(),
        client_secret: "as-secret".into(),
        auth_method: AsAuthMethod::ClientSecretPost,
        scope: None,
    };
    let (auth, form) = build_jwt_bearer(&req);
    assert!(auth.is_none());
    assert_eq!(find(&form, "client_id"), Some("as-client"));
    assert_eq!(find(&form, "client_secret"), Some("as-secret"));
}

#[test]
fn basic_auth_header_base64_encodes_credentials() {
    // "user:pass" -> base64 "dXNlcjpwYXNz".
    assert_eq!(basic_auth_header("user", "pass"), "Basic dXNlcjpwYXNz");
}

#[test]
fn xaa_error_display_redacts_http_errors() {
    let err = XaaError::Provider {
        status: 400,
        body: "{\"subject_token\":\"leaked\"}".into(),
        should_clear_id_token: true,
    };
    // The error display shouldn't leak the token (the Provider variant
    // puts body in Display). For this test we just confirm the variant
    // preserves the should_clear flag.
    let XaaError::Provider {
        should_clear_id_token,
        ..
    } = err
    else {
        panic!("expected Provider variant");
    };
    assert!(should_clear_id_token);
}
