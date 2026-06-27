use super::*;
use pretty_assertions::assert_eq;

#[test]
fn deny_unknown_fields_catches_typo() {
    let json = r#"{"orgization_id": "org-1"}"#;
    let err = serde_json::from_str::<PartialProviderClientOptions>(json).unwrap_err();
    assert!(
        err.to_string().contains("orgization_id") || err.to_string().contains("unknown field"),
        "expected unknown-field error, got: {err}"
    );
}

#[test]
fn deserialise_typed_options() {
    let json = r#"{
        "headers": {"X-Corp-Tenant": "engineering"},
        "organization_id": "org-myown",
        "include_usage": true
    }"#;
    let opts: PartialProviderClientOptions = serde_json::from_str(json).unwrap();
    assert_eq!(
        opts.headers.as_ref().unwrap().get("X-Corp-Tenant"),
        Some(&HeaderValue::literal("engineering"))
    );
    assert_eq!(opts.organization_id.as_deref(), Some("org-myown"));
    assert_eq!(opts.include_usage, Some(true));
    assert_eq!(opts.full_url, None);
}

#[test]
fn header_value_untagged_serde_both_modes() {
    // Mode 2: a bare JSON string deserialises to a literal.
    // Mode 1: a `{ "template": ... }` object deserialises to a template.
    let json = r#"{
        "headers": {
            "X-Literal": "static-value",
            "X-Templated": { "template": "${SESSION_ID}" }
        }
    }"#;
    let opts: PartialProviderClientOptions = serde_json::from_str(json).unwrap();
    let headers = opts.headers.unwrap();
    assert_eq!(
        headers.get("X-Literal"),
        Some(&HeaderValue::literal("static-value"))
    );
    assert_eq!(
        headers.get("X-Templated"),
        Some(&HeaderValue::templated("${SESSION_ID}"))
    );
    assert!(!headers["X-Literal"].is_templated());
    assert!(headers["X-Templated"].is_templated());
    assert_eq!(headers["X-Templated"].raw(), "${SESSION_ID}");
}

#[test]
fn debug_redacts_auth_token() {
    let opts = ProviderClientOptions {
        auth_token: Some(crate::secret::RedactedSecret::new("sk-bearer-token")),
        ..Default::default()
    };
    let rendered = format!("{opts:?}");
    assert!(!rendered.contains("sk-bearer-token"));
    assert!(rendered.contains("<redacted>"));
}

#[test]
fn merge_partial_overlays_each_some_field() {
    let mut base = ProviderClientOptions {
        headers: BTreeMap::from([("X-Base".into(), HeaderValue::literal("1"))]),
        organization_id: Some("org-base".into()),
        full_url: false,
        ..Default::default()
    };
    let overlay = PartialProviderClientOptions {
        headers: Some(BTreeMap::from([(
            "X-Overlay".into(),
            HeaderValue::literal("2"),
        )])),
        full_url: Some(true),
        ..Default::default()
    };
    base.merge_partial(&overlay);
    assert_eq!(base.headers.get("X-Base"), Some(&HeaderValue::literal("1")));
    assert_eq!(
        base.headers.get("X-Overlay"),
        Some(&HeaderValue::literal("2"))
    );
    assert_eq!(base.organization_id.as_deref(), Some("org-base"));
    assert!(base.full_url);
}
