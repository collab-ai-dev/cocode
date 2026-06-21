use super::*;
use std::collections::HashMap;

#[test]
fn returns_id_for_known_provider() {
    let mut r: HashMap<String, String> = HashMap::new();
    r.insert("openai".into(), "file-abc".into());
    r.insert("anthropic".into(), "file-xyz".into());
    assert_eq!(resolve_provider_reference(&r, "openai"), Some("file-abc"));
    assert_eq!(
        resolve_provider_reference(&r, "anthropic"),
        Some("file-xyz")
    );
}

#[test]
fn returns_none_for_unknown_provider() {
    let mut r: HashMap<String, String> = HashMap::new();
    r.insert("openai".into(), "file-abc".into());
    assert!(resolve_provider_reference(&r, "google").is_none());
}
