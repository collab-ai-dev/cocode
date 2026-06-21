//! DeepSeek vendor catalog — two providers (`deepseek-openai`,
//! `deepseek-anthropic`) + DeepSeek V4 models.
//!
//! DeepSeek V4 thinking surface: 3 explicit states (disable / high / max).
//! `default_thinking_level = Medium` (UX "high") — the picker entry
//! highlighted on open and the effort applied when no level is
//! explicitly selected.
//!
//!   * `Disable` — explicit off; emits `{"thinking":{"type":"disabled"}}`
//!     via `options`. Convert layer skips typed-arm emission.
//!   * `Medium`  — UX "high". Emits `{"thinking":{"type":"enabled"}}`
//!     via `options`; the OpenaiCompat arm adds `reasoning_effort: "medium"`.
//!   * `XHigh`   — UX "max". Emits `{"thinking":{"type":"enabled"}}`
//!     via `options`; the OpenaiCompat arm adds `reasoning_effort: "xhigh"`.

use std::collections::BTreeMap;
use std::collections::HashMap;

use coco_types::Capability;
use coco_types::ProviderApi;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;

use crate::EnvKey;
use crate::model::partial::PartialModelInfo;
use crate::positive::PositiveTokens;
use crate::provider::PartialProviderConfig;
use crate::provider::model_override::PartialProviderModelOverride;

pub(super) fn providers() -> Vec<(&'static str, PartialProviderConfig)> {
    vec![
        (
            "deepseek-openai",
            PartialProviderConfig {
                api: Some(ProviderApi::OpenaiCompat),
                env_key: Some(EnvKey::DeepseekApiKey.to_string()),
                // OpenAI-compatible endpoint — SDK appends `/chat/completions`.
                base_url: Some("https://api.deepseek.com/v1".into()),
                models: Some(deepseek_v4_models()),
                ..Default::default()
            },
        ),
        (
            "deepseek-anthropic",
            PartialProviderConfig {
                api: Some(ProviderApi::Anthropic),
                env_key: Some(EnvKey::DeepseekApiKey.to_string()),
                // Anthropic-compatible endpoint — must end with `/v1`; SDK
                // appends `/messages` (same rule as `api.anthropic.com/v1`).
                base_url: Some("https://api.deepseek.com/anthropic/v1".into()),
                models: Some(deepseek_v4_models()),
                ..Default::default()
            },
        ),
    ]
}

pub(super) fn models() -> Vec<(&'static str, PartialModelInfo)> {
    let thinking = deepseek_v4_thinking_levels();
    vec![
        (
            "deepseek-v4-flash",
            PartialModelInfo {
                display_name: Some("DeepSeek V4 Flash".into()),
                base_instructions: Some(super::DEFAULT_BASE_INSTRUCTIONS.into()),
                context_window: Some(PositiveTokens::new(1_000_000)),
                max_output_tokens: Some(PositiveTokens::new(12_288)),
                capabilities: Some(vec![
                    Capability::TextGeneration,
                    Capability::Streaming,
                    Capability::ToolCalling,
                    Capability::ExtendedThinking,
                    Capability::AdaptiveThinking,
                    Capability::ParallelToolCalls,
                    Capability::ClientSideToolSearch,
                ]),
                supported_thinking_levels: Some(thinking.clone()),
                default_thinking_level: Some(ReasoningEffort::Medium),
                ..Default::default()
            },
        ),
        (
            "deepseek-v4-pro",
            PartialModelInfo {
                display_name: Some("DeepSeek V4 Pro".into()),
                base_instructions: Some(super::DEFAULT_BASE_INSTRUCTIONS.into()),
                context_window: Some(PositiveTokens::new(1_000_000)),
                max_output_tokens: Some(PositiveTokens::new(12_288)),
                capabilities: Some(vec![
                    Capability::TextGeneration,
                    Capability::Streaming,
                    Capability::ToolCalling,
                    Capability::ExtendedThinking,
                    Capability::AdaptiveThinking,
                    Capability::ParallelToolCalls,
                    Capability::ClientSideToolSearch,
                ]),
                supported_thinking_levels: Some(thinking),
                default_thinking_level: Some(ReasoningEffort::Medium),
                ..Default::default()
            },
        ),
    ]
}

/// Pre-registered DeepSeek V4 model entries shared by both builtin
/// DeepSeek providers. Empty overrides — metadata comes from the
/// vendor `models()` catalog above.
fn deepseek_v4_models() -> BTreeMap<String, PartialProviderModelOverride> {
    BTreeMap::from([
        (
            "deepseek-v4-flash".into(),
            PartialProviderModelOverride::default(),
        ),
        (
            "deepseek-v4-pro".into(),
            PartialProviderModelOverride::default(),
        ),
    ])
}

fn deepseek_v4_thinking_levels() -> Vec<ThinkingLevel> {
    vec![
        ThinkingLevel {
            effort: ReasoningEffort::Off,
            budget_tokens: None,
            options: HashMap::from([(
                "thinking".to_string(),
                serde_json::json!({"type": "disabled"}),
            )]),
        },
        ThinkingLevel {
            effort: ReasoningEffort::Medium,
            budget_tokens: None,
            options: HashMap::from([(
                "thinking".to_string(),
                serde_json::json!({"type": "enabled"}),
            )]),
        },
        ThinkingLevel {
            effort: ReasoningEffort::XHigh,
            budget_tokens: None,
            options: HashMap::from([(
                "thinking".to_string(),
                serde_json::json!({"type": "enabled"}),
            )]),
        },
    ]
}
