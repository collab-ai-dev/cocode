//! Anthropic vendor catalog — `anthropic` provider + Claude models.
//!
//! Claude thinking-level budgets are aligned with
//! `vercel-ai-provider-utils::map_reasoning_to_provider_budget`
//! defaults applied to Claude's 64k `max_output_tokens` (Low 10% /
//! Medium 30% / High 60%). Declaring them here (rather than relying on
//! a provider fallback) keeps `vercel-ai-anthropic` faithful to
//! `ModelInfo`: when budget is absent the wire body omits the key
//! entirely.

use coco_types::Capability;
use coco_types::ProviderApi;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;

use crate::EnvKey;
use crate::model::partial::PartialModelInfo;
use crate::positive::PositiveTokens;
use crate::provider::PartialProviderConfig;

pub(super) fn providers() -> Vec<(&'static str, PartialProviderConfig)> {
    vec![(
        "anthropic",
        PartialProviderConfig {
            api: Some(ProviderApi::Anthropic),
            env_key: Some(EnvKey::AnthropicApiKey.to_string()),
            // **Must end with `/v1`.** `AnthropicConfig::url(path)` only
            // appends `path` (e.g. `/messages`) when `base_url` does not
            // already end with `path`; it does NOT auto-detect missing
            // version segments. So `https://api.anthropic.com` would
            // produce `/messages` (404) instead of `/v1/messages`.
            base_url: Some("https://api.anthropic.com/v1".into()),
            ..Default::default()
        },
    )]
}

pub(super) fn models() -> Vec<(&'static str, PartialModelInfo)> {
    let thinking = claude_thinking_levels();
    vec![
        (
            "claude-sonnet-4-6",
            PartialModelInfo {
                display_name: Some("Claude Sonnet 4.6".into()),
                base_instructions: Some(super::DEFAULT_BASE_INSTRUCTIONS.into()),
                context_window: Some(PositiveTokens::new(1_000_000)),
                max_output_tokens: Some(PositiveTokens::new(64_000)),
                capabilities: Some(vec![
                    Capability::TextGeneration,
                    Capability::Streaming,
                    Capability::ToolCalling,
                    Capability::Vision,
                    Capability::ExtendedThinking,
                    Capability::AdaptiveThinking,
                    Capability::FastMode,
                    Capability::PromptCache,
                    Capability::Context1m,
                    Capability::InterleavedThinking,
                    Capability::ContextManagement,
                    Capability::ParallelToolCalls,
                    Capability::ServerSideToolReference,
                    Capability::ClientSideToolSearch,
                    // Adapter `get_model_capabilities("claude-sonnet-4-6")`
                    // returns `supports_structured_output: true` →
                    // native `output_format` + `structured-outputs-2025-11-13`
                    // beta is emitted on the wire.
                    Capability::StructuredOutput,
                ]),
                supported_thinking_levels: Some(thinking.clone()),
                default_thinking_level: Some(ReasoningEffort::Medium),
                ..Default::default()
            },
        ),
        (
            "claude-opus-4-7",
            PartialModelInfo {
                display_name: Some("Claude Opus 4.7".into()),
                base_instructions: Some(super::DEFAULT_BASE_INSTRUCTIONS.into()),
                context_window: Some(PositiveTokens::new(200_000)),
                max_output_tokens: Some(PositiveTokens::new(64_000)),
                capabilities: Some(vec![
                    Capability::TextGeneration,
                    Capability::Streaming,
                    Capability::ToolCalling,
                    Capability::Vision,
                    Capability::ExtendedThinking,
                    Capability::AdaptiveThinking,
                    Capability::FastMode,
                    Capability::PromptCache,
                    Capability::InterleavedThinking,
                    Capability::ContextManagement,
                    Capability::ParallelToolCalls,
                    Capability::ServerSideToolReference,
                    Capability::ClientSideToolSearch,
                    // Anthropic adapter does not yet have an explicit
                    // entry for `claude-opus-4-7` in
                    // `get_model_capabilities`, so it falls back to the
                    // synthetic json-tool path. Recall reads
                    // `tool_uses.first()` which handles that wire shape
                    // — the capability gate still keeps the request
                    // on the structured-output rail.
                    Capability::StructuredOutput,
                ]),
                supported_thinking_levels: Some(thinking),
                default_thinking_level: Some(ReasoningEffort::Medium),
                ..Default::default()
            },
        ),
        (
            "claude-haiku-4-5",
            PartialModelInfo {
                display_name: Some("Claude Haiku 4.5".into()),
                base_instructions: Some(super::DEFAULT_BASE_INSTRUCTIONS.into()),
                context_window: Some(PositiveTokens::new(200_000)),
                max_output_tokens: Some(PositiveTokens::new(8_192)),
                // Haiku's server-side cache_read drops are noise, not real
                // prefix breaks — exclude it from cache-break detection so
                // the detector doesn't emit false positives.
                cache_break_detection_excluded: Some(true),
                capabilities: Some(vec![
                    Capability::TextGeneration,
                    Capability::Streaming,
                    Capability::ToolCalling,
                    Capability::Vision,
                    Capability::FastMode,
                    Capability::PromptCache,
                    Capability::ContextManagement,
                    Capability::ParallelToolCalls,
                    // Haiku is on TS `DEFAULT_UNSUPPORTED_MODEL_PATTERNS`
                    // so no ServerSideToolReference, but the client-side
                    // path has been validated.
                    Capability::ClientSideToolSearch,
                    // Adapter `get_model_capabilities("claude-haiku-4-5")`
                    // returns `supports_structured_output: true`.
                    Capability::StructuredOutput,
                ]),
                ..Default::default()
            },
        ),
    ]
}

fn claude_thinking_levels() -> Vec<ThinkingLevel> {
    vec![
        ThinkingLevel::with_budget(ReasoningEffort::Low, 6_400),
        ThinkingLevel::with_budget(ReasoningEffort::Medium, 19_200),
        ThinkingLevel::with_budget(ReasoningEffort::High, 38_400),
        ThinkingLevel::with_budget(ReasoningEffort::XHigh, 128_000),
    ]
}
