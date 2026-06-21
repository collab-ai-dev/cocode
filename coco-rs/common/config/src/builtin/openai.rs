//! OpenAI vendor catalog — `openai` provider + GPT-5.x models.
//!
//! GPT-5 family ships `apply_patch` as a freeform tool and excludes the
//! generic `edit` tool. The `tool_overrides` clones reuse the same
//! base instance per model entry.

use coco_types::ApplyPatchToolType;
use coco_types::Capability;
use coco_types::OAuthFlowId;
use coco_types::ProviderApi;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;
use coco_types::ToolId;
use coco_types::ToolName;
use coco_types::ToolOverrides;
use coco_types::WireApi;

use crate::model::partial::PartialModelInfo;
use crate::positive::PositiveTokens;
use crate::provider::PartialProviderConfig;
use crate::provider::ProviderAuth;

const GPT_5_4: &str = include_str!("../../instructions/gpt5_4_prompt.md");
const GPT_5_5: &str = include_str!("../../instructions/gpt5_5_prompt.md");
const GPT_5_3_CODEX: &str = include_str!("../../instructions/gpt5_3_codex_prompt.md");

pub(super) fn providers() -> Vec<(&'static str, PartialProviderConfig)> {
    vec![
        (
            "openai",
            PartialProviderConfig {
                api: Some(ProviderApi::Openai),
                env_key: Some("OPENAI_API_KEY".into()),
                base_url: Some("https://api.openai.com/v1".into()),
                // OpenAI direct defaults to the Responses API (the
                // SDK's `language_model()` default). Users with
                // legacy Chat Completions deployments override via
                // `wire_api: "chat"` in providers.json.
                wire_api: Some(WireApi::Responses),
                ..Default::default()
            },
        ),
        (
            // ChatGPT-subscription route: same OpenAI Responses wire body,
            // but authenticated by `coco login openai` (OAuth) and pointed at
            // the codex backend. `env_key` is intentionally omitted — OAuth
            // credentials come from `coco-provider-auth`, not an env var.
            super::OPENAI_CHATGPT_PROVIDER,
            PartialProviderConfig {
                api: Some(ProviderApi::Openai),
                auth: Some(ProviderAuth::OAuth {
                    flow: OAuthFlowId::OpenAiChatGpt,
                }),
                base_url: Some("https://chatgpt.com/backend-api/codex".into()),
                wire_api: Some(WireApi::Responses),
                ..Default::default()
            },
        ),
    ]
}

pub(super) fn models() -> Vec<(&'static str, PartialModelInfo)> {
    let gpt5_overrides = ToolOverrides::default()
        .with_extra(ToolId::Builtin(ToolName::ApplyPatch))
        .with_excluded(ToolId::Builtin(ToolName::Edit))
        .with_excluded(ToolId::Builtin(ToolName::Write));
    let thinking = openai_reasoning_levels();

    vec![
        (
            "gpt-5-4",
            PartialModelInfo {
                display_name: Some("GPT-5.4".into()),
                base_instructions: Some(GPT_5_4.into()),
                context_window: Some(PositiveTokens::new(272_000)),
                max_output_tokens: Some(PositiveTokens::new(12_288)),
                capabilities: Some(vec![
                    Capability::TextGeneration,
                    Capability::Streaming,
                    Capability::ToolCalling,
                    Capability::Vision,
                    Capability::StructuredOutput,
                    Capability::ExtendedThinking,
                    Capability::ReasoningSummaries,
                    Capability::ParallelToolCalls,
                    Capability::ClientSideToolSearch,
                ]),
                supported_thinking_levels: Some(thinking.clone()),
                default_thinking_level: Some(ReasoningEffort::High),
                apply_patch_tool_type: Some(ApplyPatchToolType::Freeform),
                tool_overrides: Some(gpt5_overrides.clone()),
                ..Default::default()
            },
        ),
        (
            "gpt-5-5",
            PartialModelInfo {
                display_name: Some("GPT-5.5".into()),
                base_instructions: Some(GPT_5_5.into()),
                context_window: Some(PositiveTokens::new(272_000)),
                max_output_tokens: Some(PositiveTokens::new(12_288)),
                capabilities: Some(vec![
                    Capability::TextGeneration,
                    Capability::Streaming,
                    Capability::ToolCalling,
                    Capability::Vision,
                    Capability::StructuredOutput,
                    Capability::ExtendedThinking,
                    Capability::ReasoningSummaries,
                    Capability::ParallelToolCalls,
                    Capability::ClientSideToolSearch,
                ]),
                supported_thinking_levels: Some(thinking.clone()),
                default_thinking_level: Some(ReasoningEffort::High),
                apply_patch_tool_type: Some(ApplyPatchToolType::Freeform),
                tool_overrides: Some(gpt5_overrides.clone()),
                ..Default::default()
            },
        ),
        (
            "gpt-5-3-codex",
            PartialModelInfo {
                display_name: Some("GPT-5.3 Codex".into()),
                base_instructions: Some(GPT_5_3_CODEX.into()),
                context_window: Some(PositiveTokens::new(272_000)),
                max_output_tokens: Some(PositiveTokens::new(12_288)),
                capabilities: Some(vec![
                    Capability::TextGeneration,
                    Capability::Streaming,
                    Capability::ToolCalling,
                    Capability::Vision,
                    Capability::StructuredOutput,
                    Capability::ExtendedThinking,
                    Capability::ReasoningSummaries,
                    Capability::ParallelToolCalls,
                    Capability::ClientSideToolSearch,
                ]),
                supported_thinking_levels: Some(thinking),
                default_thinking_level: Some(ReasoningEffort::High),
                apply_patch_tool_type: Some(ApplyPatchToolType::Freeform),
                tool_overrides: Some(gpt5_overrides),
                ..Default::default()
            },
        ),
    ]
}

fn openai_reasoning_levels() -> Vec<ThinkingLevel> {
    vec![
        ThinkingLevel::disable(),
        ThinkingLevel::low(),
        ThinkingLevel::medium(),
        ThinkingLevel::high(),
        ThinkingLevel::xhigh(),
    ]
}
