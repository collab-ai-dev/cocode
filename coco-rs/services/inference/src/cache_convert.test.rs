use super::*;
use coco_types::BetaCapability;
use coco_types::CacheScope;
use coco_types::CacheTtl;
use pretty_assertions::assert_eq;
use std::collections::BTreeSet;

fn auto_one_hour() -> PromptCacheConfig {
    PromptCacheConfig {
        mode: PromptCacheMode::Auto,
        ttl: CacheTtl::OneHour,
        scope: Some(CacheScope::Org),
        requested_betas: BTreeSet::new(),
        skip_cache_write: false,
    }
}

#[test]
fn anthropic_emits_camelcase_keys() {
    let cfg = auto_one_hour();
    let out = to_extra_body(&cfg, ProviderApi::Anthropic);
    assert!(out.contains_key("cacheStrategy"));
    assert!(!out.contains_key("cache_strategy"));
}

#[test]
fn cache_strategy_round_trips_all_fields() {
    let cfg = auto_one_hour();
    let out = to_extra_body(&cfg, ProviderApi::Anthropic);
    let strat = out.get("cacheStrategy").unwrap();
    assert_eq!(strat.get("mode"), Some(&serde_json::json!("auto")));
    assert_eq!(strat.get("ttl"), Some(&serde_json::json!("one_hour")));
    assert_eq!(strat.get("scope"), Some(&serde_json::json!("org")));
    assert_eq!(strat.get("skipCacheWrite"), Some(&serde_json::json!(false)));
}

#[test]
fn requested_betas_emitted_only_when_non_empty() {
    let mut cfg = auto_one_hour();
    let out = to_extra_body(&cfg, ProviderApi::Anthropic);
    assert!(!out.contains_key("requestedBetas"));

    cfg.requested_betas.insert(BetaCapability::Context1m);
    let out = to_extra_body(&cfg, ProviderApi::Anthropic);
    assert_eq!(
        out.get("requestedBetas"),
        Some(&serde_json::json!(["context_1m"]))
    );
}

#[test]
fn non_anthropic_emits_empty_map() {
    let cfg = auto_one_hour();
    for api in [
        ProviderApi::Openai,
        ProviderApi::Gemini,
        ProviderApi::Volcengine,
        ProviderApi::Zai,
        ProviderApi::OpenaiCompat,
    ] {
        let out = to_extra_body(&cfg, api);
        assert!(
            out.is_empty(),
            "{api:?} must not see prompt-cache keys (multi-provider isolation)"
        );
    }
}

#[test]
fn disabled_mode_emits_empty_map() {
    let cfg = PromptCacheConfig::default(); // Disabled
    let out = to_extra_body(&cfg, ProviderApi::Anthropic);
    assert!(out.is_empty());
}

#[test]
fn session_context_writes_agentic_and_query_source() {
    let cfg = auto_one_hour();
    let out = session_context_to_extra_body(
        Some(&cfg),
        /*agentic*/ true,
        Some("repl_main_thread"),
        ProviderApi::Anthropic,
    );
    assert_eq!(out.get("agenticQuery"), Some(&serde_json::json!(true)));
    assert_eq!(
        out.get("querySource"),
        Some(&serde_json::json!("repl_main_thread"))
    );
    // Account / overage are NOT carried per-call (R3-F3 — session-stable on AnthropicConfig).
    assert!(!out.contains_key("accountKind"));
    assert!(!out.contains_key("inOverage"));
    // No userType / entrypoint either (§7.1 — Ant gates not ported).
    assert!(!out.contains_key("userType"));
    assert!(!out.contains_key("entrypoint"));
}

#[test]
fn session_context_skipped_when_strategy_disabled() {
    // Finding 4 fix: query_source change for a caller without prompt
    // caching MUST NOT re-hash extra_body. Verified by the empty map.
    let cfg = PromptCacheConfig::default(); // Disabled
    let out = session_context_to_extra_body(
        Some(&cfg),
        /*agentic*/ true,
        Some("repl_main_thread"),
        ProviderApi::Anthropic,
    );
    assert!(
        out.is_empty(),
        "session context must not emit when cache strategy is disabled"
    );
}

#[test]
fn session_context_skipped_when_cache_cfg_absent() {
    let out = session_context_to_extra_body(
        None,
        /*agentic*/ true,
        Some("repl_main_thread"),
        ProviderApi::Anthropic,
    );
    assert!(out.is_empty());
}

#[test]
fn session_context_skipped_for_non_anthropic_even_with_active_strategy() {
    let cfg = auto_one_hour();
    for api in [ProviderApi::Openai, ProviderApi::Gemini] {
        let out = session_context_to_extra_body(
            Some(&cfg),
            /*agentic*/ true,
            Some("repl_main_thread"),
            api,
        );
        assert!(out.is_empty(), "{api:?} must not see session context keys");
    }
}

#[test]
fn session_context_emits_agentic_without_query_source() {
    let cfg = auto_one_hour();
    let out = session_context_to_extra_body(
        Some(&cfg),
        /*agentic*/ false,
        None,
        ProviderApi::Anthropic,
    );
    assert_eq!(out.get("agenticQuery"), Some(&serde_json::json!(false)));
    assert!(!out.contains_key("querySource"));
}
