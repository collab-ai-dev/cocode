use pretty_assertions::assert_eq;
use std::collections::BTreeSet;

use super::*;

#[test]
fn prompt_cache_mode_serde_roundtrip() {
    for (mode, wire) in [
        (PromptCacheMode::Disabled, "\"disabled\""),
        (PromptCacheMode::Auto, "\"auto\""),
        (PromptCacheMode::Manual, "\"manual\""),
    ] {
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, wire);
        let parsed: PromptCacheMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, mode);
    }
}

#[test]
fn prompt_cache_mode_default_is_disabled() {
    assert_eq!(PromptCacheMode::default(), PromptCacheMode::Disabled);
}

#[test]
fn cache_ttl_serde_roundtrip() {
    for (ttl, wire) in [
        (CacheTtl::FiveMinutes, "\"five_minutes\""),
        (CacheTtl::OneHour, "\"one_hour\""),
    ] {
        let json = serde_json::to_string(&ttl).unwrap();
        assert_eq!(json, wire);
        let parsed: CacheTtl = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ttl);
    }
}

#[test]
fn cache_scope_serde_roundtrip() {
    for (scope, wire) in [
        (CacheScope::Org, "\"org\""),
        (CacheScope::Global, "\"global\""),
    ] {
        let json = serde_json::to_string(&scope).unwrap();
        assert_eq!(json, wire);
        let parsed: CacheScope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, scope);
    }
}

#[test]
fn beta_capability_serde_uses_snake_case() {
    let cap = BetaCapability::Context1m;
    let json = serde_json::to_string(&cap).unwrap();
    assert_eq!(json, "\"context_1m\"");

    let cap = BetaCapability::PromptCachingScope;
    let json = serde_json::to_string(&cap).unwrap();
    assert_eq!(json, "\"prompt_caching_scope\"");
}

#[test]
fn account_kind_default_is_api_key() {
    assert_eq!(AccountKind::default(), AccountKind::ApiKey);
}

#[test]
fn account_kind_serde_roundtrip() {
    for (kind, wire) in [
        (AccountKind::ApiKey, "\"api_key\""),
        (AccountKind::ClaudeAiSubscriber, "\"claude_ai_subscriber\""),
    ] {
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, wire);
        let parsed: AccountKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }
}

#[test]
fn prompt_cache_config_default_is_disabled_no_betas() {
    let cfg = PromptCacheConfig::default();
    assert_eq!(cfg.mode, PromptCacheMode::Disabled);
    assert_eq!(cfg.ttl, CacheTtl::FiveMinutes);
    assert_eq!(cfg.scope, None);
    assert!(cfg.requested_betas.is_empty());
    assert!(!cfg.skip_cache_write);
}

#[test]
fn prompt_cache_config_skips_empty_optional_fields_on_serialize() {
    let cfg = PromptCacheConfig::default();
    let json = serde_json::to_value(&cfg).unwrap();
    let obj = json.as_object().unwrap();
    assert!(!obj.contains_key("scope"), "empty scope must be omitted");
    assert!(
        !obj.contains_key("requested_betas"),
        "empty requested_betas must be omitted"
    );
}

#[test]
fn prompt_cache_config_full_roundtrip() {
    let mut requested = BTreeSet::new();
    requested.insert(BetaCapability::Context1m);
    requested.insert(BetaCapability::FastMode);
    let cfg = PromptCacheConfig {
        mode: PromptCacheMode::Auto,
        ttl: CacheTtl::OneHour,
        scope: Some(CacheScope::Global),
        requested_betas: requested,
        skip_cache_write: true,
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: PromptCacheConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, cfg);
}

#[test]
fn capability_includes_new_prompt_cache_variants() {
    use crate::Capability;
    for cap in [
        Capability::PromptCache,
        Capability::Context1m,
        Capability::InterleavedThinking,
        Capability::ContextManagement,
        Capability::TokenEfficientTools,
    ] {
        let json = serde_json::to_string(&cap).unwrap();
        let parsed: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, cap);
    }
}

#[test]
fn capability_prompt_cache_wire_format_is_snake_case() {
    use crate::Capability;
    assert_eq!(
        serde_json::to_string(&Capability::PromptCache).unwrap(),
        "\"prompt_cache\""
    );
    assert_eq!(
        serde_json::to_string(&Capability::Context1m).unwrap(),
        "\"context_1m\""
    );
    assert_eq!(
        serde_json::to_string(&Capability::TokenEfficientTools).unwrap(),
        "\"token_efficient_tools\""
    );
    assert_eq!(
        serde_json::to_string(&Capability::ServerSideToolReference).unwrap(),
        "\"server_side_tool_reference\""
    );
    assert_eq!(
        serde_json::to_string(&Capability::ClientSideToolSearch).unwrap(),
        "\"client_side_tool_search\""
    );
}
