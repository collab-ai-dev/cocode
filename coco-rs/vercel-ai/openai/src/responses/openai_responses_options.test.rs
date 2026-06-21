use super::*;
use serde_json::json;
use std::collections::HashMap;
use vercel_ai_provider::ProviderOptions;

/// Extras carry unknown keys verbatim; typed-consumed keys
/// (`reasoningSummary` is parsed into `reasoning_summary` and emitted
/// in its canonical wire location) stay out, so the inference-layer
/// injection of camelCase reasoning signals via
/// `provider_options["openai"]` can no longer leak to the body root.
#[test]
fn extras_carry_unknown_keys_but_not_typed_keys() {
    let mut inner = HashMap::new();
    inner.insert("reasoningSummary".into(), json!("auto")); // typed-known
    inner.insert("myCustom".into(), json!(123)); // unknown
    let mut outer = HashMap::new();
    outer.insert("openai".into(), inner);
    let po = Some(ProviderOptions(outer));

    let (typed, raw) = extract_responses_options(&po);
    assert_eq!(typed.reasoning_summary.as_deref(), Some("auto"));
    // typed-consumed key is gone from raw; unknown key remains.
    assert!(!raw.contains_key("reasoningSummary"));
    assert!(raw.contains_key("myCustom"));
}
