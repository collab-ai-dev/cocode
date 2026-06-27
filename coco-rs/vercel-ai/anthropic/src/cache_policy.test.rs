use super::*;
use crate::anthropic_config::AdapterAccountKind;
use crate::anthropic_config::AnthropicConfig;
use crate::anthropic_config::AnthropicModelCapabilities;
use crate::anthropic_config::ProviderTopology;
use crate::messages::anthropic_messages_options::AdapterCacheMode;
use crate::messages::anthropic_messages_options::AdapterCacheTtl;
use crate::messages::anthropic_messages_options::CacheStrategy;
use std::collections::HashMap;
use std::sync::Arc;

fn make_config(
    account_kind: AdapterAccountKind,
    in_overage: bool,
    allowlist: Vec<String>,
) -> AnthropicConfig {
    AnthropicConfig {
        provider: "anthropic.messages".into(),
        base_url: "https://api.anthropic.com/v1".into(),
        headers: Arc::new(HashMap::new),
        client: None,
        supports_native_structured_output: None,
        supports_strict_tools: None,
        full_url: None,
        capabilities: AnthropicModelCapabilities::default(),
        provider_topology: ProviderTopology::FirstParty,
        experimental_betas_enabled: true,
        disable_interleaved_thinking: false,
        show_thinking_summaries: false,
        non_interactive: false,
        prompt_cache_allowlist: allowlist,
        account_kind,
        in_overage,
    }
}

fn strategy(mode: AdapterCacheMode, ttl: AdapterCacheTtl) -> CacheStrategy {
    CacheStrategy {
        mode,
        ttl,
        scope: None,
        skip_cache_write: false,
    }
}

#[test]
fn disabled_strategy_returns_none() {
    let policy = CachePolicy::new();
    let config = make_config(AdapterAccountKind::ApiKey, false, vec![]);
    let result = policy.resolve_ttl(
        &config,
        &strategy(AdapterCacheMode::Disabled, AdapterCacheTtl::OneHour),
        Some("main"),
    );
    assert_eq!(result, None);
}

#[test]
fn five_min_strategy_returns_five_min_without_eligibility_check() {
    let policy = CachePolicy::new();
    // Even subscriber + no overage gets 5m as requested.
    let config = make_config(AdapterAccountKind::ClaudeAiSubscriber, false, vec![]);
    let result = policy.resolve_ttl(
        &config,
        &strategy(AdapterCacheMode::Auto, AdapterCacheTtl::FiveMinutes),
        None,
    );
    assert_eq!(result, Some(AdapterCacheTtl::FiveMinutes));
}

#[test]
fn one_hour_for_api_key_with_matching_allowlist() {
    let policy = CachePolicy::new();
    let config = make_config(AdapterAccountKind::ApiKey, false, vec!["main".into()]);
    let result = policy.resolve_ttl(
        &config,
        &strategy(AdapterCacheMode::Auto, AdapterCacheTtl::OneHour),
        Some("main"),
    );
    assert_eq!(result, Some(AdapterCacheTtl::OneHour));
}

#[test]
fn one_hour_downgraded_to_five_min_when_allowlist_misses() {
    let policy = CachePolicy::new();
    let config = make_config(AdapterAccountKind::ApiKey, false, vec!["agent_*".into()]);
    let result = policy.resolve_ttl(
        &config,
        &strategy(AdapterCacheMode::Auto, AdapterCacheTtl::OneHour),
        Some("compaction"),
    );
    assert_eq!(result, Some(AdapterCacheTtl::FiveMinutes));
}

#[test]
fn one_hour_subscriber_without_overage_downgrades() {
    let policy = CachePolicy::new();
    let config = make_config(
        AdapterAccountKind::ClaudeAiSubscriber,
        /*in_overage*/ false,
        vec!["main".into()],
    );
    let result = policy.resolve_ttl(
        &config,
        &strategy(AdapterCacheMode::Auto, AdapterCacheTtl::OneHour),
        Some("main"),
    );
    assert_eq!(result, Some(AdapterCacheTtl::FiveMinutes));
}

#[test]
fn one_hour_subscriber_with_overage_keeps_one_hour() {
    let policy = CachePolicy::new();
    let config = make_config(
        AdapterAccountKind::ClaudeAiSubscriber,
        /*in_overage*/ true,
        vec!["main".into()],
    );
    let result = policy.resolve_ttl(
        &config,
        &strategy(AdapterCacheMode::Auto, AdapterCacheTtl::OneHour),
        Some("main"),
    );
    assert_eq!(result, Some(AdapterCacheTtl::OneHour));
}

#[test]
fn allowlist_glob_match() {
    let policy = CachePolicy::new();
    let config = make_config(AdapterAccountKind::ApiKey, false, vec!["agent_*".into()]);
    let result = policy.resolve_ttl(
        &config,
        &strategy(AdapterCacheMode::Auto, AdapterCacheTtl::OneHour),
        Some("agent_review"),
    );
    assert_eq!(result, Some(AdapterCacheTtl::OneHour));
}

#[test]
fn missing_query_source_treated_as_no_match() {
    let policy = CachePolicy::new();
    let config = make_config(AdapterAccountKind::ApiKey, false, vec!["main".into()]);
    let result = policy.resolve_ttl(
        &config,
        &strategy(AdapterCacheMode::Auto, AdapterCacheTtl::OneHour),
        None,
    );
    assert_eq!(result, Some(AdapterCacheTtl::FiveMinutes));
}

#[test]
fn eligibility_latched_after_first_call() {
    let policy = CachePolicy::new();
    // First call with subscriber + overage → eligible.
    let initial = make_config(
        AdapterAccountKind::ClaudeAiSubscriber,
        true,
        vec!["m".into()],
    );
    let r1 = policy.resolve_ttl(
        &initial,
        &strategy(AdapterCacheMode::Auto, AdapterCacheTtl::OneHour),
        Some("m"),
    );
    assert_eq!(r1, Some(AdapterCacheTtl::OneHour));

    // Second call with same policy but a config that pretends overage flipped:
    // latch must stick — TS R3-F3 session-stable property.
    let flipped = make_config(
        AdapterAccountKind::ClaudeAiSubscriber,
        false,
        vec!["m".into()],
    );
    let r2 = policy.resolve_ttl(
        &flipped,
        &strategy(AdapterCacheMode::Auto, AdapterCacheTtl::OneHour),
        Some("m"),
    );
    assert_eq!(r2, Some(AdapterCacheTtl::OneHour));
}

#[test]
fn allowlist_latched_after_first_call() {
    let policy = CachePolicy::new();
    let initial = make_config(AdapterAccountKind::ApiKey, false, vec!["main".into()]);
    let _ = policy.resolve_ttl(
        &initial,
        &strategy(AdapterCacheMode::Auto, AdapterCacheTtl::OneHour),
        Some("main"),
    );
    // Now flip allowlist; latched snapshot wins.
    let flipped = make_config(AdapterAccountKind::ApiKey, false, vec!["other".into()]);
    let r2 = policy.resolve_ttl(
        &flipped,
        &strategy(AdapterCacheMode::Auto, AdapterCacheTtl::OneHour),
        Some("main"),
    );
    assert_eq!(r2, Some(AdapterCacheTtl::OneHour));
}
