use super::*;
use serde_json::json;

#[test]
fn empty_map_returns_typed_defaults() {
    let resolved = parse_provider_options(&BTreeMap::new()).expect("infallible for empty");
    assert_eq!(resolved, AnthropicProviderOptionsConfig::default());
    assert!(resolved.experimental_betas_enabled);
    assert!(!resolved.disable_interleaved_thinking);
    assert!(!resolved.show_thinking_summaries);
    assert!(!resolved.non_interactive);
}

#[test]
fn explicit_false_overrides_default_true() {
    let mut map = BTreeMap::new();
    map.insert("experimental_betas".into(), json!(false));
    let resolved = parse_provider_options(&map).expect("valid");
    assert!(!resolved.experimental_betas_enabled);
    // Other fields stay at their defaults.
    assert!(!resolved.disable_interleaved_thinking);
}

#[test]
fn partial_map_fills_unset_with_defaults() {
    let mut map = BTreeMap::new();
    map.insert("disable_interleaved_thinking".into(), json!(true));
    map.insert("non_interactive".into(), json!(true));
    let resolved = parse_provider_options(&map).expect("valid");
    assert!(resolved.experimental_betas_enabled); // default
    assert!(resolved.disable_interleaved_thinking); // set
    assert!(!resolved.show_thinking_summaries); // default
    assert!(resolved.non_interactive); // set
}

#[test]
fn unknown_key_is_rejected_at_parse_time() {
    let mut map = BTreeMap::new();
    map.insert("disable_interleaved_thinkin".into(), json!(true)); // typo
    let err = parse_provider_options(&map).expect_err("must reject typos");
    let msg = err.to_string();
    assert!(
        msg.contains("disable_interleaved_thinkin"),
        "error should name the typo'd key, got: {msg}"
    );
}

#[test]
fn wrong_type_is_rejected_at_parse_time() {
    let mut map = BTreeMap::new();
    map.insert("experimental_betas".into(), json!("yes")); // string, not bool
    let err = parse_provider_options(&map).expect_err("must reject wrong type");
    let msg = err.to_string();
    assert!(
        msg.contains("experimental_betas") || msg.contains("bool"),
        "error should reference the field or expected type, got: {msg}"
    );
}

#[test]
fn all_four_keys_set_round_trip() {
    let mut map = BTreeMap::new();
    map.insert("experimental_betas".into(), json!(false));
    map.insert("disable_interleaved_thinking".into(), json!(true));
    map.insert("show_thinking_summaries".into(), json!(true));
    map.insert("non_interactive".into(), json!(true));
    let resolved = parse_provider_options(&map).expect("valid");
    assert_eq!(
        resolved,
        AnthropicProviderOptionsConfig {
            experimental_betas_enabled: false,
            disable_interleaved_thinking: true,
            show_thinking_summaries: true,
            non_interactive: true,
        }
    );
}
