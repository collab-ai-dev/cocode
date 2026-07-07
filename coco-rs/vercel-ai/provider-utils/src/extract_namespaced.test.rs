use super::*;
use pretty_assertions::assert_eq;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct FakeOpts {
    thinking_level: Option<String>,
    temperature: Option<f64>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

impl ExtractExtras for FakeOpts {
    fn take_extras(&mut self) -> BTreeMap<String, Value> {
        std::mem::take(&mut self.extra)
    }
}

fn po_with(entries: &[(&str, Value)]) -> ProviderOptions {
    let mut po = ProviderOptions::new();
    for (ns, body) in entries {
        let map: HashMap<String, Value> = body
            .as_object()
            .expect("test fixture expects an object")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        po.set(*ns, map);
    }
    po
}

#[test]
fn returns_default_when_provider_options_none() {
    let extracted =
        extract_namespaced::<FakeOpts>(None, "google", "google").expect("valid provider options");
    assert_eq!(extracted.typed, FakeOpts::default());
    assert!(extracted.extras.is_empty());
}

#[test]
fn returns_default_when_namespace_absent() {
    let po = po_with(&[("anthropic", json!({"thinkingLevel": "high"}))]);
    let extracted = extract_namespaced::<FakeOpts>(Some(&po), "google", "google")
        .expect("valid provider options");
    assert_eq!(extracted.typed, FakeOpts::default());
    assert!(extracted.extras.is_empty());
}

#[test]
fn canonical_only_typed_and_extras_split() {
    let po = po_with(&[(
        "google",
        json!({
            "thinkingLevel": "medium",
            "temperature": 0.5,
            "extraKey": "extraVal",
        }),
    )]);
    let extracted = extract_namespaced::<FakeOpts>(Some(&po), "google", "google")
        .expect("valid provider options");
    assert_eq!(extracted.typed.thinking_level.as_deref(), Some("medium"));
    assert_eq!(extracted.typed.temperature, Some(0.5));
    assert_eq!(extracted.extras.get("extraKey"), Some(&json!("extraVal")));
    assert!(!extracted.extras.contains_key("thinkingLevel"));
    assert!(!extracted.extras.contains_key("temperature"));
}

#[test]
fn custom_overrides_canonical_at_per_key_deep_merge() {
    let po = po_with(&[
        (
            "google",
            json!({"thinkingLevel": "low", "temperature": 0.7}),
        ),
        // Custom overrides only thinkingLevel; temperature inherits.
        ("vertex", json!({"thinkingLevel": "high"})),
    ]);
    let extracted = extract_namespaced::<FakeOpts>(Some(&po), "google", "vertex")
        .expect("valid provider options");
    assert_eq!(extracted.typed.thinking_level.as_deref(), Some("high"));
    assert_eq!(extracted.typed.temperature, Some(0.7));
    assert!(extracted.extras.is_empty());
}

#[test]
fn custom_only_typed_and_extras_split() {
    let po = po_with(&[(
        "vertex",
        json!({"thinkingLevel": "high", "vertexOnly": "x"}),
    )]);
    let extracted = extract_namespaced::<FakeOpts>(Some(&po), "google", "vertex")
        .expect("valid provider options");
    assert_eq!(extracted.typed.thinking_level.as_deref(), Some("high"));
    assert_eq!(extracted.extras.get("vertexOnly"), Some(&json!("x")));
}

#[test]
fn extras_deep_merge_per_key_when_both_namespaces_have_them() {
    let po = po_with(&[
        (
            "google",
            json!({"nested": {"a": 1, "b": 2}, "soloCanonical": 10}),
        ),
        ("vertex", json!({"nested": {"b": 99}, "soloCustom": 20})),
    ]);
    let extracted = extract_namespaced::<FakeOpts>(Some(&po), "google", "vertex")
        .expect("valid provider options");
    assert_eq!(
        extracted.extras.get("nested"),
        Some(&json!({"a": 1, "b": 99}))
    );
    assert_eq!(extracted.extras.get("soloCanonical"), Some(&json!(10)));
    assert_eq!(extracted.extras.get("soloCustom"), Some(&json!(20)));
}

#[test]
fn malformed_typed_field_returns_error() {
    let po = po_with(&[("google", json!({"thinkingLevel": 42, "extraKey": "v"}))]);
    let err = extract_namespaced::<FakeOpts>(Some(&po), "google", "google")
        .expect_err("malformed typed provider option must fail");
    assert_eq!(err.namespace, "google");
    assert_eq!(err.field_path, "thinkingLevel");
    assert!(err.value_summary.contains("\"thinkingLevel\":42"));
}

#[test]
fn malformed_nested_custom_namespace_reports_custom_namespace() {
    let po = po_with(&[
        ("google", json!({"thinkingLevel": "low"})),
        ("vertex", json!({"temperature": {"bad": true}})),
    ]);
    let err = extract_namespaced::<FakeOpts>(Some(&po), "google", "vertex")
        .expect_err("malformed custom typed provider option must fail");
    assert_eq!(err.namespace, "vertex");
    assert_eq!(err.field_path, "temperature");
}
