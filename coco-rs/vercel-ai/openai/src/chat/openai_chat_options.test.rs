use super::*;
use serde_json::json;
use std::collections::HashMap;
use vercel_ai_provider::ProviderOptions;

/// `raw` (the extras map) carries unknown keys verbatim so users can
/// push extra_body fields, while typed-consumed keys (`user`, etc.)
/// stay out — they're already placed in their canonical wire location
/// by `get_args`, and re-emitting them at the body root would let
/// internally-injected camelCase signals (e.g. `reasoningSummary`)
/// leak there as well. Mirrors the Google adapter's `extra` field.
#[test]
fn extras_carry_unknown_keys_but_not_typed_keys() {
    let mut inner = HashMap::new();
    inner.insert("user".into(), json!("uid")); // typed-known
    inner.insert("myCustom".into(), json!(true)); // unknown
    let mut outer = HashMap::new();
    outer.insert("openai".into(), inner);
    let po = Some(ProviderOptions(outer));

    let (typed, raw) = extract_openai_options(&po);
    assert_eq!(typed.user.as_deref(), Some("uid"));
    // typed-consumed key is gone from raw; unknown key remains.
    assert!(!raw.contains_key("user"));
    assert!(raw.contains_key("myCustom"));
}
