use super::*;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct OpenAIOpts {
    reasoning_effort: Option<String>,
    user: Option<String>,
}

fn opts_with(provider: &str, body: serde_json::Value) -> ProviderOptions {
    let mut o = ProviderOptions::new();
    let map: HashMap<String, serde_json::Value> = body
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    o.set(provider, map);
    o
}

#[test]
fn returns_none_when_no_options() {
    let r: Option<OpenAIOpts> = parse_provider_options("openai", None).unwrap();
    assert!(r.is_none());
}

#[test]
fn returns_none_when_namespace_absent() {
    let opts = opts_with("anthropic", serde_json::json!({"thinking": {}}));
    let r: Option<OpenAIOpts> = parse_provider_options("openai", Some(&opts)).unwrap();
    assert!(r.is_none());
}

#[test]
fn returns_some_when_namespace_present() {
    let opts = opts_with(
        "openai",
        serde_json::json!({"reasoningEffort": "high", "user": "u123"}),
    );
    let r: Option<OpenAIOpts> = parse_provider_options("openai", Some(&opts)).unwrap();
    assert_eq!(
        r,
        Some(OpenAIOpts {
            reasoning_effort: Some("high".into()),
            user: Some("u123".into()),
        })
    );
}

#[test]
fn errors_on_invalid_shape() {
    // reasoningEffort must be a string; passing a number breaks deser.
    let opts = opts_with("openai", serde_json::json!({"reasoningEffort": 42}));
    let r: Result<Option<OpenAIOpts>, _> = parse_provider_options("openai", Some(&opts));
    assert!(r.is_err());
}

#[test]
fn fallback_picks_secondary_when_primary_missing() {
    let opts = opts_with("openaiCompatible", serde_json::json!({"user": "fallback"}));
    let r: Option<OpenAIOpts> =
        parse_provider_options_with_fallback("xai", &["openaiCompatible"], Some(&opts)).unwrap();
    assert_eq!(
        r,
        Some(OpenAIOpts {
            reasoning_effort: None,
            user: Some("fallback".into()),
        })
    );
}

#[test]
fn fallback_prefers_primary_over_fallback() {
    let mut opts = opts_with("xai", serde_json::json!({"user": "primary"}));
    let mut fallback_inner = HashMap::new();
    fallback_inner.insert("user".to_string(), serde_json::json!("fallback"));
    opts.set("openaiCompatible", fallback_inner);
    let r: Option<OpenAIOpts> =
        parse_provider_options_with_fallback("xai", &["openaiCompatible"], Some(&opts)).unwrap();
    assert_eq!(r.unwrap().user, Some("primary".into()));
}
