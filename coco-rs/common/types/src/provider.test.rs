use pretty_assertions::assert_eq;

use super::*;

#[test]
fn test_provider_api_serde_roundtrip() {
    let provider = ProviderApi::Anthropic;
    let json = serde_json::to_string(&provider).unwrap();
    assert_eq!(json, "\"anthropic\"");
    let parsed: ProviderApi = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, provider);
}

#[test]
fn test_capability_set() {
    let mut caps = CapabilitySet::new();
    caps.insert(Capability::Vision);
    caps.insert(Capability::ToolCalling);
    assert!(caps.contains(&Capability::Vision));
    assert!(!caps.contains(&Capability::Audio));
}

#[test]
fn test_model_spec_equality_ignores_display_name() {
    let a = ModelSpec {
        provider: "anthropic".into(),
        api: ProviderApi::Anthropic,
        model_id: "claude-opus-4-6".into(),
        display_name: "Claude Opus 4.6".into(),
    };
    let b = ModelSpec {
        provider: "anthropic".into(),
        api: ProviderApi::Anthropic,
        model_id: "claude-opus-4-6".into(),
        display_name: "Different Name".into(),
    };
    assert_eq!(a, b);
}

#[test]
fn test_apply_patch_tool_type_default() {
    assert_eq!(ApplyPatchToolType::default(), ApplyPatchToolType::Freeform);
}

#[test]
fn test_model_role_as_str_matches_serde() {
    for role in [
        ModelRole::Main,
        ModelRole::Fast,
        ModelRole::Plan,
        ModelRole::Explore,
        ModelRole::Review,
        ModelRole::HookAgent,
        ModelRole::Memory,
        ModelRole::Subagent,
    ] {
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, format!("\"{}\"", role.as_str()));
    }
}

#[test]
fn test_model_role_from_str_canonical() {
    assert_eq!("main".parse::<ModelRole>().unwrap(), ModelRole::Main);
    assert_eq!("explore".parse::<ModelRole>().unwrap(), ModelRole::Explore);
    assert_eq!(
        "hook_agent".parse::<ModelRole>().unwrap(),
        ModelRole::HookAgent
    );
}

#[test]
fn test_model_role_from_str_accepts_camelcase_and_whitespace() {
    assert_eq!(
        "hookAgent".parse::<ModelRole>().unwrap(),
        ModelRole::HookAgent
    );
    assert_eq!("  Plan  ".parse::<ModelRole>().unwrap(), ModelRole::Plan);
    assert_eq!("EXPLORE".parse::<ModelRole>().unwrap(), ModelRole::Explore);
}

#[test]
fn test_model_role_from_str_rejects_unknown() {
    assert!("nope".parse::<ModelRole>().is_err());
    assert!("teammate".parse::<ModelRole>().is_err());
    assert!("".parse::<ModelRole>().is_err());
}

/// `OAuthFlowId::as_str` MUST equal the serde wire form for every variant —
/// the fingerprint digest and persisted `StoredCredential.flow` would otherwise
/// disagree. Mirrors `test_model_role_as_str_matches_serde`.
#[test]
fn test_oauth_flow_id_as_str_matches_serde() {
    for flow in [OAuthFlowId::OpenAiChatGpt, OAuthFlowId::GeminiCodeAssist] {
        let json = serde_json::to_string(&flow).unwrap();
        assert_eq!(json, format!("\"{}\"", flow.as_str()));
        // Round-trips back from the canonical spelling.
        let parsed: OAuthFlowId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, flow);
    }
}
