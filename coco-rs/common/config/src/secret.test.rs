use super::*;

#[test]
fn debug_does_not_leak_inner() {
    let secret = RedactedSecret::new("sk-1234567890abcdef");
    let rendered = format!("{secret:?}");
    assert_eq!(rendered, "RedactedSecret(<redacted>)");
    assert!(!rendered.contains("sk-1234567890abcdef"));
}

#[test]
fn display_does_not_leak_inner() {
    let secret = RedactedSecret::new("sk-abc");
    assert_eq!(format!("{secret}"), "<redacted>");
    assert!(!format!("{secret}").contains("sk-abc"));
}

#[test]
fn expose_returns_inner_value() {
    let secret = RedactedSecret::new("sk-real-key");
    assert_eq!(secret.expose(), "sk-real-key");
}

#[test]
fn deserialise_from_transparent_string() {
    let secret: RedactedSecret = serde_json::from_str(r#""sk-from-json""#).unwrap();
    assert_eq!(secret.expose(), "sk-from-json");
}

#[test]
fn serialise_round_trips_inner_value() {
    let secret = RedactedSecret::new("sk-roundtrip");
    let serialised = serde_json::to_string(&secret).unwrap();
    assert_eq!(serialised, r#""sk-roundtrip""#);
}
