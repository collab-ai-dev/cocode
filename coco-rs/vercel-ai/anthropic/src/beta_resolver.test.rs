use super::*;
use crate::anthropic_config::AdapterAccountKind;
use crate::anthropic_config::AnthropicConfig;
use crate::anthropic_config::AnthropicModelCapabilities;
use crate::anthropic_config::ProviderTopology;
use std::collections::HashMap;
use std::sync::Arc;

fn make_config(
    capabilities: AnthropicModelCapabilities,
    experimental: bool,
    disable_isp: bool,
    show_summaries: bool,
    non_interactive: bool,
) -> AnthropicConfig {
    AnthropicConfig {
        provider: "anthropic.messages".into(),
        base_url: "https://api.anthropic.com/v1".into(),
        headers: Arc::new(HashMap::new),
        client: None,
        supports_native_structured_output: None,
        supports_strict_tools: None,
        full_url: None,
        capabilities,
        provider_topology: ProviderTopology::FirstParty,
        experimental_betas_enabled: experimental,
        disable_interleaved_thinking: disable_isp,
        show_thinking_summaries: show_summaries,
        non_interactive,
        prompt_cache_allowlist: vec![],
        account_kind: AdapterAccountKind::ApiKey,
        in_overage: false,
    }
}

fn all_caps_on() -> AnthropicModelCapabilities {
    AnthropicModelCapabilities {
        prompt_cache: true,
        context_1m: true,
        interleaved_thinking: true,
        context_management: true,
        token_efficient_tools: true,
        tool_reference: true,
    }
}

#[test]
fn agentic_query_emits_baseline() {
    let config = make_config(
        AnthropicModelCapabilities::default(),
        true,
        false,
        false,
        false,
    );
    let resolved = resolve(&config, /*agentic*/ true, &[]);
    assert!(resolved.headers.contains("claude-code-20250219"));
}

#[test]
fn helper_call_skips_baseline() {
    let config = make_config(
        AnthropicModelCapabilities::default(),
        true,
        false,
        false,
        false,
    );
    let resolved = resolve(&config, /*agentic*/ false, &[]);
    assert!(!resolved.headers.contains("claude-code-20250219"));
}

#[test]
fn requested_betas_top_up() {
    let config = make_config(
        AnthropicModelCapabilities::default(),
        true,
        false,
        false,
        false,
    );
    let resolved = resolve(
        &config,
        false,
        &[
            AdapterBetaCapability::Advisor,
            AdapterBetaCapability::FastMode,
        ],
    );
    assert!(resolved.headers.contains("advisor-2025-12-04"));
    assert!(resolved.headers.contains("fast-mode-2026-02-01"));
}

#[test]
fn context_1m_emitted_on_capable_model() {
    let caps = AnthropicModelCapabilities {
        context_1m: true,
        ..Default::default()
    };
    let config = make_config(caps, true, false, false, false);
    let resolved = resolve(&config, false, &[]);
    assert!(resolved.headers.contains("context-1m-2025-08-07"));
}

#[test]
fn tool_search_beta_emitted_on_capable_model() {
    // `tool-search-tool-2025-10-19` is a capability-only gate — no
    // topology / experimental / agentic-query filter.
    let caps = AnthropicModelCapabilities {
        tool_reference: true,
        ..Default::default()
    };
    let config = make_config(caps, false, false, false, false);
    let resolved = resolve(&config, false, &[]);
    assert!(
        resolved.headers.contains("tool-search-tool-2025-10-19"),
        "expected tool-search beta when tool_reference cap is on: {:?}",
        resolved.headers
    );
}

#[test]
fn tool_search_beta_suppressed_when_capability_off() {
    // Sanity: incapable model (e.g. Haiku, older Claude 3) must not
    // emit the beta header — server would 400 on unrecognized header.
    let config = make_config(
        AnthropicModelCapabilities::default(),
        true,
        false,
        false,
        false,
    );
    let resolved = resolve(&config, true, &[]);
    assert!(!resolved.headers.contains("tool-search-tool-2025-10-19"));
}

#[test]
fn interleaved_thinking_suppressed_by_disable_flag() {
    let config = make_config(
        all_caps_on(),
        /*experimental*/ true,
        /*disable_isp*/ true,
        false,
        false,
    );
    let resolved = resolve(&config, false, &[]);
    assert!(!resolved.headers.contains("interleaved-thinking-2025-05-14"));
}

#[test]
fn redact_thinking_suppressed_when_show_summaries() {
    let config = make_config(
        all_caps_on(),
        /*experimental*/ true,
        /*disable_isp*/ false,
        /*show_summaries*/ true,
        false,
    );
    let resolved = resolve(&config, false, &[]);
    assert!(!resolved.headers.contains("redact-thinking-2026-02-12"));
}

#[test]
fn redact_thinking_suppressed_when_non_interactive() {
    let config = make_config(
        all_caps_on(),
        true,
        false,
        false,
        /*non_interactive*/ true,
    );
    let resolved = resolve(&config, false, &[]);
    assert!(!resolved.headers.contains("redact-thinking-2026-02-12"));
}

#[test]
fn context_management_predicate_matches_emitted_header() {
    let on = make_config(all_caps_on(), true, false, false, false);
    let off = make_config(
        all_caps_on(),
        /*experimental*/ false,
        false,
        false,
        false,
    );
    assert!(should_emit_context_management(&on));
    assert!(!should_emit_context_management(&off));
    let r_on = resolve(&on, false, &[]);
    let r_off = resolve(&off, false, &[]);
    assert!(r_on.headers.contains("context-management-2025-06-27"));
    assert!(!r_off.headers.contains("context-management-2025-06-27"));
    assert!(r_on.context_management_eligible);
    assert!(!r_off.context_management_eligible);
}

#[test]
fn prompt_caching_scope_requires_experimental_gate() {
    let off = make_config(
        AnthropicModelCapabilities::default(),
        false,
        false,
        false,
        false,
    );
    let on = make_config(
        AnthropicModelCapabilities::default(),
        true,
        false,
        false,
        false,
    );
    let r_off = resolve(&off, false, &[]);
    let r_on = resolve(&on, false, &[]);
    assert!(!r_off.headers.contains("prompt-caching-scope-2026-01-05"));
    assert!(r_on.headers.contains("prompt-caching-scope-2026-01-05"));
}

#[test]
fn header_set_is_sorted_for_determinism() {
    let config = make_config(all_caps_on(), true, false, false, false);
    let resolved = resolve(
        &config,
        true,
        &[
            AdapterBetaCapability::Advisor,
            AdapterBetaCapability::FastMode,
        ],
    );
    let collected: Vec<&String> = resolved.headers.iter().collect();
    let mut sorted = collected.clone();
    sorted.sort();
    assert_eq!(collected, sorted);
}
