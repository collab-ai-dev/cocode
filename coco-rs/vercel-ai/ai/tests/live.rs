//! Live integration tests for vercel-ai providers.
//!
//! These tests run against real LLM provider APIs and require credentials
//! configured in `.env.test` or via environment variables.
//!
//! # Test Organization
//!
//! Tests are organized in a parameterized suite structure where the same test
//! logic runs against multiple providers. Each test function follows the pattern:
//! `test_{feature}_{provider}`.
//!
//! OpenAI is tested via two explicit API variants:
//! - `openai_chat` — Chat Completions API (`/v1/chat/completions`)
//! - `openai_responses` — Responses API (`/v1/responses`)
//!
//! Both variants auto-derive credentials from `VERCEL_AI_TEST_OPENAI_*` env vars.
//!
//! # Running Tests
//!
//! ```bash
//! # Run all integration tests (all configured providers)
//! cargo test -p vercel-ai --test live -- --test-threads=1
//!
//! # Run all tests for a specific provider
//! cargo test -p vercel-ai --test live openai_chat -- --test-threads=1
//! cargo test -p vercel-ai --test live openai_responses -- --test-threads=1
//! cargo test -p vercel-ai --test live openai_compatible -- --test-threads=1
//! cargo test -p vercel-ai --test live anthropic -- --test-threads=1
//!
//! # Run specific test category
//! cargo test -p vercel-ai --test live test_basic -- --test-threads=1
//! cargo test -p vercel-ai --test live test_tools -- --test-threads=1
//!
//! # Run specific provider + feature
//! cargo test -p vercel-ai --test live test_basic_openai_chat -- --test-threads=1
//! cargo test -p vercel-ai --test live test_streaming_anthropic -- --test-threads=1
//! ```
//!
//! # Configuration
//!
//! Set environment variables for each provider:
//! - `VERCEL_AI_TEST_{PROVIDER}_API_KEY` - Required
//! - `VERCEL_AI_TEST_{PROVIDER}_MODEL` - Required
//! - `VERCEL_AI_TEST_{PROVIDER}_BASE_URL` - Optional
//!
//! Or use a `.env.test` file in the crate root.
//!
//! # Capability Gating
//!
//! Control which test categories run via environment variables:
//!
//! ```bash
//! # Global: only run text and streaming tests for all providers
//! VERCEL_AI_TEST_CAPABILITIES=text,streaming
//!
//! # Per-provider: override global setting
//! VERCEL_AI_TEST_OPENAI_CAPABILITIES=text,streaming,tools,vision
//! VERCEL_AI_TEST_ANTHROPIC_CAPABILITIES=text,tools
//!
//! # Per-variant: override for a specific OpenAI API variant
//! VERCEL_AI_TEST_OPENAI_CHAT_CAPABILITIES=text,streaming,tools
//! VERCEL_AI_TEST_OPENAI_RESPONSES_CAPABILITIES=text,streaming,tools,vision
//! ```
//!
//! Available capabilities: `text`, `streaming`, `tools`, `vision`, `cross_provider`.
//! If no capability env vars are set, all capabilities are enabled by default.

mod common;

// Test suite modules
#[path = "live/suite/mod.rs"]
mod suite;

use anyhow::Result;

// ============================================================================
// Helper Macro for Test Generation
// ============================================================================

/// Macro to generate a test function for a specific provider.
///
/// Usage:
/// - `provider_test!(provider_name, test_fn)` — no capability check
/// - `provider_test!(provider_name, "capability", test_fn)` — with capability gate
macro_rules! provider_test {
    ($provider:expr, $test_fn:path) => {{
        let (_provider, model) = require_provider!($provider);
        $test_fn(&model).await
    }};
    ($provider:expr, $capability:expr, $test_fn:path) => {{
        let (_provider, model) = require_provider!($provider, $capability);
        $test_fn(&model).await
    }};
}

// ============================================================================
// Basic Text Generation Tests
// ============================================================================

#[tokio::test]
async fn test_basic_openai_chat() -> Result<()> {
    provider_test!("openai_chat", "text", suite::basic::run)
}

#[tokio::test]
async fn test_basic_openai_responses() -> Result<()> {
    provider_test!("openai_responses", "text", suite::basic::run)
}

#[tokio::test]
async fn test_basic_openai_compatible() -> Result<()> {
    provider_test!("openai_compatible", "text", suite::basic::run)
}

#[tokio::test]
async fn test_basic_anthropic() -> Result<()> {
    provider_test!("anthropic", "text", suite::basic::run)
}

#[tokio::test]
async fn test_basic_google() -> Result<()> {
    provider_test!("google", "text", suite::basic::run)
}

// ============================================================================
// Token Usage Tests
// ============================================================================

#[tokio::test]
async fn test_token_usage_openai_chat() -> Result<()> {
    provider_test!("openai_chat", "text", suite::basic::run_token_usage)
}

#[tokio::test]
async fn test_token_usage_openai_responses() -> Result<()> {
    provider_test!("openai_responses", "text", suite::basic::run_token_usage)
}

#[tokio::test]
async fn test_token_usage_openai_compatible() -> Result<()> {
    provider_test!("openai_compatible", "text", suite::basic::run_token_usage)
}

#[tokio::test]
async fn test_token_usage_anthropic() -> Result<()> {
    provider_test!("anthropic", "text", suite::basic::run_token_usage)
}

#[tokio::test]
async fn test_token_usage_google() -> Result<()> {
    provider_test!("google", "text", suite::basic::run_token_usage)
}

// ============================================================================
// Multi-Turn Conversation Tests
// ============================================================================

#[tokio::test]
async fn test_multi_turn_openai_chat() -> Result<()> {
    provider_test!("openai_chat", "text", suite::basic::run_multi_turn)
}

#[tokio::test]
async fn test_multi_turn_openai_responses() -> Result<()> {
    provider_test!("openai_responses", "text", suite::basic::run_multi_turn)
}

#[tokio::test]
async fn test_multi_turn_openai_compatible() -> Result<()> {
    provider_test!("openai_compatible", "text", suite::basic::run_multi_turn)
}

#[tokio::test]
async fn test_multi_turn_anthropic() -> Result<()> {
    provider_test!("anthropic", "text", suite::basic::run_multi_turn)
}

#[tokio::test]
async fn test_multi_turn_google() -> Result<()> {
    provider_test!("google", "text", suite::basic::run_multi_turn)
}

// ============================================================================
// Tool Calling Tests
// ============================================================================

#[tokio::test]
async fn test_tools_openai_chat() -> Result<()> {
    provider_test!("openai_chat", "tools", suite::tools::run)
}

#[tokio::test]
async fn test_tools_openai_responses() -> Result<()> {
    provider_test!("openai_responses", "tools", suite::tools::run)
}

#[tokio::test]
async fn test_tools_openai_compatible() -> Result<()> {
    provider_test!("openai_compatible", "tools", suite::tools::run)
}

#[tokio::test]
async fn test_tools_anthropic() -> Result<()> {
    provider_test!("anthropic", "tools", suite::tools::run)
}

#[tokio::test]
async fn test_tools_google() -> Result<()> {
    provider_test!("google", "tools", suite::tools::run)
}

// ============================================================================
// Tool Complete Flow Tests
// ============================================================================

#[tokio::test]
async fn test_tool_flow_openai_chat() -> Result<()> {
    provider_test!("openai_chat", "tools", suite::tools::run_complete_flow)
}

#[tokio::test]
async fn test_tool_flow_openai_responses() -> Result<()> {
    provider_test!("openai_responses", "tools", suite::tools::run_complete_flow)
}

#[tokio::test]
async fn test_tool_flow_openai_compatible() -> Result<()> {
    provider_test!(
        "openai_compatible",
        "tools",
        suite::tools::run_complete_flow
    )
}

#[tokio::test]
async fn test_tool_flow_anthropic() -> Result<()> {
    provider_test!("anthropic", "tools", suite::tools::run_complete_flow)
}

#[tokio::test]
async fn test_tool_flow_google() -> Result<()> {
    provider_test!("google", "tools", suite::tools::run_complete_flow)
}

// ============================================================================
// Streaming Tests
// ============================================================================

#[tokio::test]
async fn test_streaming_openai_chat() -> Result<()> {
    provider_test!("openai_chat", "streaming", suite::streaming::run)
}

#[tokio::test]
async fn test_streaming_openai_responses() -> Result<()> {
    provider_test!("openai_responses", "streaming", suite::streaming::run)
}

#[tokio::test]
async fn test_streaming_openai_compatible() -> Result<()> {
    provider_test!("openai_compatible", "streaming", suite::streaming::run)
}

#[tokio::test]
async fn test_streaming_anthropic() -> Result<()> {
    provider_test!("anthropic", "streaming", suite::streaming::run)
}

#[tokio::test]
async fn test_streaming_google() -> Result<()> {
    provider_test!("google", "streaming", suite::streaming::run)
}

// ============================================================================
// Streaming with Tools Tests
// ============================================================================

#[tokio::test]
async fn test_streaming_tools_openai_chat() -> Result<()> {
    provider_test!("openai_chat", "tools", suite::streaming::run_with_tools)
}

#[tokio::test]
async fn test_streaming_tools_openai_responses() -> Result<()> {
    provider_test!(
        "openai_responses",
        "tools",
        suite::streaming::run_with_tools
    )
}

#[tokio::test]
async fn test_streaming_tools_openai_compatible() -> Result<()> {
    provider_test!(
        "openai_compatible",
        "tools",
        suite::streaming::run_with_tools
    )
}

#[tokio::test]
async fn test_streaming_tools_anthropic() -> Result<()> {
    provider_test!("anthropic", "tools", suite::streaming::run_with_tools)
}

#[tokio::test]
async fn test_streaming_tools_google() -> Result<()> {
    provider_test!("google", "tools", suite::streaming::run_with_tools)
}

// ============================================================================
// Vision / Image Understanding Tests
// ============================================================================

#[tokio::test]
async fn test_vision_openai_chat() -> Result<()> {
    provider_test!("openai_chat", "vision", suite::vision::run)
}

#[tokio::test]
async fn test_vision_openai_responses() -> Result<()> {
    provider_test!("openai_responses", "vision", suite::vision::run)
}

#[tokio::test]
async fn test_vision_anthropic() -> Result<()> {
    provider_test!("anthropic", "vision", suite::vision::run)
}

#[tokio::test]
async fn test_vision_google() -> Result<()> {
    provider_test!("google", "vision", suite::vision::run)
}

// ============================================================================
// Cross-Provider Conversation Tests
// ============================================================================
//
// Dynamically discovers all configured providers with the `cross_provider`
// capability and tests all ordered pairs (source -> target).

#[tokio::test]
async fn test_cross_provider_all() -> Result<()> {
    let providers = common::config::list_cross_provider_configs();
    if providers.len() < 2 {
        eprintln!(
            "Skipping cross-provider tests: need >= 2 providers with cross_provider capability, \
             found {} ({:?})",
            providers.len(),
            providers.iter().map(|c| &c.provider).collect::<Vec<_>>()
        );
        return Ok(());
    }

    eprintln!(
        "Running cross-provider tests for {} providers: {:?}",
        providers.len(),
        providers.iter().map(|c| &c.provider).collect::<Vec<_>>()
    );

    let mut failures = vec![];
    for (i, source) in providers.iter().enumerate() {
        for (j, target) in providers.iter().enumerate() {
            if i == j {
                continue;
            }
            eprintln!("  {} -> {} ...", source.provider, target.provider);
            let (_, src_model) = match common::create_provider_and_model(source) {
                Some(pair) => pair,
                None => {
                    failures.push(format!(
                        "{} -> {}: failed to create source provider",
                        source.provider, target.provider
                    ));
                    continue;
                }
            };
            let (_, tgt_model) = match common::create_provider_and_model(target) {
                Some(pair) => pair,
                None => {
                    failures.push(format!(
                        "{} -> {}: failed to create target provider",
                        source.provider, target.provider
                    ));
                    continue;
                }
            };
            if let Err(e) = suite::cross_provider::run(&src_model, &tgt_model).await {
                failures.push(format!("{} -> {}: {e}", source.provider, target.provider));
            } else {
                eprintln!("  {} -> {} ok", source.provider, target.provider);
            }
        }
    }

    assert!(
        failures.is_empty(),
        "Cross-provider failures:\n{}",
        failures.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_cross_provider_round_trip_all() -> Result<()> {
    let providers = common::config::list_cross_provider_configs();
    if providers.len() < 2 {
        eprintln!("Skipping round-trip cross-provider tests: need >= 2 providers");
        return Ok(());
    }

    eprintln!(
        "Running A->B->A round-trip cross-provider tests for {} providers: {:?}",
        providers.len(),
        providers.iter().map(|c| &c.provider).collect::<Vec<_>>()
    );

    let mut failures = vec![];
    for (i, provider_a) in providers.iter().enumerate() {
        for (j, provider_b) in providers.iter().enumerate() {
            if i == j {
                continue;
            }
            eprintln!(
                "  {} -> {} -> {} ...",
                provider_a.provider, provider_b.provider, provider_a.provider
            );
            let (_, model_a) = match common::create_provider_and_model(provider_a) {
                Some(pair) => pair,
                None => {
                    failures.push(format!(
                        "{} -> {} -> {}: failed to create provider A",
                        provider_a.provider, provider_b.provider, provider_a.provider
                    ));
                    continue;
                }
            };
            let (_, model_b) = match common::create_provider_and_model(provider_b) {
                Some(pair) => pair,
                None => {
                    failures.push(format!(
                        "{} -> {} -> {}: failed to create provider B",
                        provider_a.provider, provider_b.provider, provider_a.provider
                    ));
                    continue;
                }
            };
            if let Err(e) = suite::cross_provider::run_round_trip(&model_a, &model_b).await {
                failures.push(format!(
                    "{} -> {} -> {}: {e}",
                    provider_a.provider, provider_b.provider, provider_a.provider
                ));
            } else {
                eprintln!(
                    "  {} -> {} -> {} ok",
                    provider_a.provider, provider_b.provider, provider_a.provider
                );
            }
        }
    }

    assert!(
        failures.is_empty(),
        "Round-trip cross-provider failures:\n{}",
        failures.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_cross_provider_tools_all() -> Result<()> {
    let providers = common::config::list_cross_provider_configs();
    // Also need tools capability
    let providers: Vec<_> = providers
        .into_iter()
        .filter(|c| c.has_capability("tools"))
        .collect();
    if providers.len() < 2 {
        eprintln!(
            "Skipping cross-provider tool tests: need >= 2 providers with tools+cross_provider"
        );
        return Ok(());
    }

    eprintln!(
        "Running cross-provider tool tests for {} providers: {:?}",
        providers.len(),
        providers.iter().map(|c| &c.provider).collect::<Vec<_>>()
    );

    let tools = common::weather_tool_registry();

    let mut failures = vec![];
    for (i, provider_a) in providers.iter().enumerate() {
        for (j, provider_b) in providers.iter().enumerate() {
            if i == j {
                continue;
            }
            eprintln!(
                "  tools {} -> {} ...",
                provider_a.provider, provider_b.provider
            );
            let (_, model_a) = match common::create_provider_and_model(provider_a) {
                Some(pair) => pair,
                None => {
                    failures.push(format!(
                        "tools {} -> {}: failed to create provider A",
                        provider_a.provider, provider_b.provider
                    ));
                    continue;
                }
            };
            let (_, model_b) = match common::create_provider_and_model(provider_b) {
                Some(pair) => pair,
                None => {
                    failures.push(format!(
                        "tools {} -> {}: failed to create provider B",
                        provider_a.provider, provider_b.provider
                    ));
                    continue;
                }
            };
            if let Err(e) =
                suite::cross_provider::run_with_tools(&model_a, &model_b, tools.clone()).await
            {
                failures.push(format!(
                    "tools {} -> {}: {e}",
                    provider_a.provider, provider_b.provider
                ));
            } else {
                eprintln!(
                    "  tools {} -> {} ok",
                    provider_a.provider, provider_b.provider
                );
            }
        }
    }

    assert!(
        failures.is_empty(),
        "Cross-provider tool failures:\n{}",
        failures.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_cross_provider_streaming_all() -> Result<()> {
    let providers = common::config::list_cross_provider_configs();
    if providers.len() < 2 {
        eprintln!("Skipping streaming cross-provider tests: need >= 2 providers");
        return Ok(());
    }

    eprintln!(
        "Running streaming cross-provider tests for {} providers: {:?}",
        providers.len(),
        providers.iter().map(|c| &c.provider).collect::<Vec<_>>()
    );

    let mut failures = vec![];
    for (i, source) in providers.iter().enumerate() {
        for (j, target) in providers.iter().enumerate() {
            if i == j {
                continue;
            }
            eprintln!("  streaming {} -> {} ...", source.provider, target.provider);
            let (_, src_model) = match common::create_provider_and_model(source) {
                Some(pair) => pair,
                None => {
                    failures.push(format!(
                        "{} -> {}: failed to create source provider",
                        source.provider, target.provider
                    ));
                    continue;
                }
            };
            let (_, tgt_model) = match common::create_provider_and_model(target) {
                Some(pair) => pair,
                None => {
                    failures.push(format!(
                        "{} -> {}: failed to create target provider",
                        source.provider, target.provider
                    ));
                    continue;
                }
            };
            if let Err(e) = suite::cross_provider::run_streaming(&src_model, &tgt_model).await {
                failures.push(format!(
                    "streaming {} -> {}: {e}",
                    source.provider, target.provider
                ));
            } else {
                eprintln!("  streaming {} -> {} ok", source.provider, target.provider);
            }
        }
    }

    assert!(
        failures.is_empty(),
        "Streaming cross-provider failures:\n{}",
        failures.join("\n")
    );
    Ok(())
}

// ============================================================================
// Configuration Tests
// ============================================================================

#[test]
fn test_list_configured_providers() {
    let providers = common::config::list_configured_providers();
    eprintln!("Configured providers: {providers:?}");
}
