use pretty_assertions::assert_eq;

use super::SubagentRuntimeSnapshot;
use crate::{ProviderApi, WireApi};

#[test]
fn test_subagent_runtime_snapshot_serde_roundtrip_anthropic() {
    let snap = SubagentRuntimeSnapshot {
        provider: "anthropic-prod".into(),
        api: ProviderApi::Anthropic,
        api_model_name: "claude-opus-4-7".into(),
        base_url: "https://api.anthropic.com".into(),
        wire_api: None,
    };
    let json = serde_json::to_string(&snap).unwrap();
    let back: SubagentRuntimeSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(snap, back);
    // wire_api None must skip on serialize.
    assert!(
        !json.contains("wire_api"),
        "Anthropic should omit wire_api: {json}"
    );
}

#[test]
fn test_subagent_runtime_snapshot_openai_carries_wire_api() {
    let snap = SubagentRuntimeSnapshot {
        provider: "openai-prod".into(),
        api: ProviderApi::Openai,
        api_model_name: "gpt-5".into(),
        base_url: "https://api.openai.com/v1".into(),
        wire_api: Some(WireApi::Responses),
    };
    let json = serde_json::to_string(&snap).unwrap();
    assert!(json.contains("\"wire_api\":\"responses\""), "got: {json}");
    let back: SubagentRuntimeSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(snap, back);
}

#[test]
fn test_subagent_runtime_snapshot_eq_distinguishes_providers() {
    // Two providers using the same Anthropic API must not compare
    // equal — `provider` is part of the identity.
    let prod = SubagentRuntimeSnapshot {
        provider: "anthropic-prod".into(),
        api: ProviderApi::Anthropic,
        api_model_name: "claude-opus-4-7".into(),
        base_url: "https://api.anthropic.com".into(),
        wire_api: None,
    };
    let dev = SubagentRuntimeSnapshot {
        provider: "anthropic-dev".into(),
        ..prod.clone()
    };
    assert_ne!(prod, dev);
}

#[test]
fn test_subagent_runtime_snapshot_eq_distinguishes_base_urls() {
    // Same provider+model on different base URLs (region / proxy)
    // must compare unequal.
    let region_a = SubagentRuntimeSnapshot {
        provider: "openai".into(),
        api: ProviderApi::Openai,
        api_model_name: "gpt-5".into(),
        base_url: "https://us.openai.example".into(),
        wire_api: Some(WireApi::Chat),
    };
    let region_b = SubagentRuntimeSnapshot {
        base_url: "https://eu.openai.example".into(),
        ..region_a.clone()
    };
    assert_ne!(region_a, region_b);
}
