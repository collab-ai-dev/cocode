use super::*;
use serde_json::json;
use std::collections::HashMap;
use vercel_ai_provider::ProviderOptions;

fn options_with(map: HashMap<String, HashMap<String, serde_json::Value>>) -> ProviderOptions {
    ProviderOptions(map)
}

/// Extras carry unknown keys verbatim; typed-consumed keys
/// (e.g. `disableParallelToolUse`) stay out — they're handled by the
/// typed path and re-emitting them at the body root would let
/// internally-injected signals (formerly the
/// `INTERNAL_ANTHROPIC_OPTION_KEYS` blacklist of `cacheStrategy` /
/// `requestedBetas` / `agenticQuery` / `querySource`) leak there too.
/// `#[serde(flatten)] extra` is the structural replacement for the
/// blacklist.
#[test]
fn extras_carry_unknown_keys_but_not_typed_keys() {
    let mut inner = HashMap::new();
    inner.insert("disableParallelToolUse".into(), json!(true)); // typed-known
    inner.insert("myCustomField".into(), json!("x")); // unknown
    inner.insert("anotherKey".into(), json!(42)); // unknown
    let mut outer = HashMap::new();
    outer.insert("anthropic".into(), inner);
    let provider_options = Some(options_with(outer));

    let (typed, raw) = extract_anthropic_options(&provider_options, "anthropic.messages");
    assert_eq!(typed.disable_parallel_tool_use, Some(true));

    // Typed-consumed key absent; unknown keys remain.
    assert!(!raw.contains_key("disableParallelToolUse"));
    assert!(raw.contains_key("myCustomField"));
    assert!(raw.contains_key("anotherKey"));
}

/// Locks down the structural replacement for the
/// `INTERNAL_ANTHROPIC_OPTION_KEYS` blacklist: the four formerly-
/// blacklisted internal signals are typed fields, so they're auto-
/// stripped from extras without any explicit filter. (Spot-checks the
/// two simpler signals; `cacheStrategy` + `requestedBetas` have
/// their own typed coverage via `cache_convert` integration tests.)
#[test]
fn internal_signals_never_leak_into_extras() {
    let mut inner = HashMap::new();
    inner.insert("agenticQuery".into(), json!(true));
    inner.insert("querySource".into(), json!("main_loop"));
    inner.insert("myCustomField".into(), json!("ok"));
    let mut outer = HashMap::new();
    outer.insert("anthropic".into(), inner);
    let provider_options = Some(options_with(outer));

    let (typed, raw) = extract_anthropic_options(&provider_options, "anthropic.messages");
    // Typed fields received the internal signals.
    assert_eq!(typed.agentic_query, Some(true));
    assert_eq!(typed.query_source.as_deref(), Some("main_loop"));
    // Extras do NOT carry them — replaces the old blacklist
    // (`INTERNAL_ANTHROPIC_OPTION_KEYS`).
    assert!(!raw.contains_key("agenticQuery"));
    assert!(!raw.contains_key("querySource"));
    // Unknown keys still pass through to extras for deep-merge.
    assert!(raw.contains_key("myCustomField"));
}

#[test]
fn raw_is_empty_when_no_provider_options() {
    let (typed, raw) = extract_anthropic_options(&None, "anthropic.messages");
    assert!(raw.is_empty());
    assert!(typed.thinking.is_none());
}

#[test]
fn raw_merges_canonical_and_custom_namespace() {
    // Canonical "anthropic" + custom "my-proxy" namespace each
    // contribute keys to `raw`. Custom wins per-key on conflict;
    // both contribute to typed parsing too.
    let mut canonical = HashMap::new();
    canonical.insert("xCanonical".into(), json!("from-canonical"));
    canonical.insert("shared".into(), json!("canonical-value"));
    let mut custom = HashMap::new();
    custom.insert("xCustom".into(), json!("from-custom"));
    custom.insert("shared".into(), json!("custom-value"));
    let mut outer = HashMap::new();
    outer.insert("anthropic".into(), canonical);
    outer.insert("my-proxy".into(), custom);
    let provider_options = Some(options_with(outer));

    let (_typed, raw) = extract_anthropic_options(&provider_options, "my-proxy.messages");
    assert_eq!(raw.get("xCanonical"), Some(&json!("from-canonical")));
    assert_eq!(raw.get("xCustom"), Some(&json!("from-custom")));
    assert_eq!(
        raw.get("shared"),
        Some(&json!("custom-value")),
        "custom namespace wins per-key on conflict"
    );
}
