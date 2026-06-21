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
    let (typed, extras) = extract_namespaced::<FakeOpts>(None, "google", "google");
    assert_eq!(typed, FakeOpts::default());
    assert!(extras.is_empty());
}

#[test]
fn returns_default_when_namespace_absent() {
    let po = po_with(&[("anthropic", json!({"thinkingLevel": "high"}))]);
    let (typed, extras) = extract_namespaced::<FakeOpts>(Some(&po), "google", "google");
    assert_eq!(typed, FakeOpts::default());
    assert!(extras.is_empty());
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
    let (typed, extras) = extract_namespaced::<FakeOpts>(Some(&po), "google", "google");
    assert_eq!(typed.thinking_level.as_deref(), Some("medium"));
    assert_eq!(typed.temperature, Some(0.5));
    assert_eq!(extras.get("extraKey"), Some(&json!("extraVal")));
    assert!(!extras.contains_key("thinkingLevel"));
    assert!(!extras.contains_key("temperature"));
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
    let (typed, extras) = extract_namespaced::<FakeOpts>(Some(&po), "google", "vertex");
    assert_eq!(typed.thinking_level.as_deref(), Some("high"));
    assert_eq!(typed.temperature, Some(0.7));
    assert!(extras.is_empty());
}

#[test]
fn custom_only_typed_and_extras_split() {
    let po = po_with(&[(
        "vertex",
        json!({"thinkingLevel": "high", "vertexOnly": "x"}),
    )]);
    let (typed, extras) = extract_namespaced::<FakeOpts>(Some(&po), "google", "vertex");
    assert_eq!(typed.thinking_level.as_deref(), Some("high"));
    assert_eq!(extras.get("vertexOnly"), Some(&json!("x")));
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
    let (_typed, extras) = extract_namespaced::<FakeOpts>(Some(&po), "google", "vertex");
    assert_eq!(extras.get("nested"), Some(&json!({"a": 1, "b": 99})));
    assert_eq!(extras.get("soloCanonical"), Some(&json!(10)));
    assert_eq!(extras.get("soloCustom"), Some(&json!(20)));
}

#[test]
fn typo_in_typed_field_falls_back_to_default_silently() {
    // F9 TODO: when thinkingLevel is the wrong shape, the entire
    // typed parse fails and falls back to default. Extras also empty
    // because take_extras runs on the default-constructed struct.
    // This lock prevents accidental change in the "tolerant" direction
    // before F9 is properly implemented.
    let po = po_with(&[("google", json!({"thinkingLevel": 42, "extraKey": "v"}))]);
    let (typed, extras) = extract_namespaced::<FakeOpts>(Some(&po), "google", "google");
    assert_eq!(typed, FakeOpts::default());
    assert!(extras.is_empty());
}
