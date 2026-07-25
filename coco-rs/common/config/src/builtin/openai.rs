//! OpenAI vendor catalog — `openai` provider + GPT-5.x models.
//!
//! GPT-5 family ships `apply_patch` as a freeform tool and excludes the
//! generic `edit` tool. The `tool_overrides` clones reuse the same
//! base instance per model entry.
//!
//! Catalog ids are dashed (`gpt-5-6-sol`); the wire slug is dotted
//! (`gpt-5.6-sol`) and is carried by `api_model_name` — see
//! [`openai_gpt5_models`].

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

use std::collections::BTreeMap;

use crate::model::partial::PartialModelInfo;
use crate::positive::PositiveTokens;
use crate::provider::PartialProviderConfig;
use crate::provider::ProviderAuth;
use crate::provider::model_override::PartialProviderModelOverride;

const GPT_5_4: &str = include_str!("../../instructions/gpt5_4_prompt.md");
const GPT_5_5: &str = include_str!("../../instructions/gpt5_5_prompt.md");
/// One prompt for the whole 5.6 family — the vendor ships identical base
/// instructions for `sol`, `terra`, and `luna`; only the reasoning ladder
/// and defaults differ between them.
const GPT_5_6: &str = include_str!("../../instructions/gpt5_6_prompt.md");
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
                models: Some(openai_gpt5_models()),
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
                models: Some(openai_gpt5_models()),
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
    // 5.6 capabilities are identical across the family; only the ladder
    // and the default rung differ, so the vec is built once and cloned.
    let gpt56_capabilities = vec![
        Capability::TextGeneration,
        Capability::Streaming,
        Capability::ToolCalling,
        Capability::Vision,
        Capability::StructuredOutput,
        Capability::ExtendedThinking,
        Capability::ReasoningSummaries,
        Capability::ParallelToolCalls,
        Capability::OpenAiNativeToolSearch,
    ];

    vec![
        (
            // `sol` — frontier tier. Vendor default effort is `low`: the
            // family is tuned to be strong at the cheap rungs, so starting
            // low and escalating is the intended usage, not a downgrade.
            "gpt-5-6-sol",
            PartialModelInfo {
                display_name: Some("GPT-5.6 Sol".into()),
                base_instructions: Some(super::render_instruction_template(GPT_5_6)),
                context_window: Some(PositiveTokens::new(272_000)),
                max_output_tokens: Some(PositiveTokens::new(12_288)),
                capabilities: Some(gpt56_capabilities.clone()),
                supported_thinking_levels: Some(gpt56_reasoning_levels(UltraSupport::Yes)),
                default_thinking_level: Some(ReasoningEffort::Low),
                apply_patch_tool_type: Some(ApplyPatchToolType::Freeform),
                tool_overrides: Some(gpt5_overrides.clone()),
                extra_body: Some(gpt56_extra_body()),
                ..Default::default()
            },
        ),
        (
            // `terra` — balanced tier, same surface as `sol` with a
            // medium default.
            "gpt-5-6-terra",
            PartialModelInfo {
                display_name: Some("GPT-5.6 Terra".into()),
                base_instructions: Some(super::render_instruction_template(GPT_5_6)),
                context_window: Some(PositiveTokens::new(272_000)),
                max_output_tokens: Some(PositiveTokens::new(12_288)),
                capabilities: Some(gpt56_capabilities.clone()),
                supported_thinking_levels: Some(gpt56_reasoning_levels(UltraSupport::Yes)),
                default_thinking_level: Some(ReasoningEffort::Medium),
                apply_patch_tool_type: Some(ApplyPatchToolType::Freeform),
                tool_overrides: Some(gpt5_overrides.clone()),
                extra_body: Some(gpt56_extra_body()),
                ..Default::default()
            },
        ),
        (
            // `luna` — fast/affordable tier. The vendor catalog stops its
            // ladder at `max`; `ultra` is not offered here.
            "gpt-5-6-luna",
            PartialModelInfo {
                display_name: Some("GPT-5.6 Luna".into()),
                base_instructions: Some(super::render_instruction_template(GPT_5_6)),
                context_window: Some(PositiveTokens::new(272_000)),
                max_output_tokens: Some(PositiveTokens::new(12_288)),
                capabilities: Some(gpt56_capabilities),
                supported_thinking_levels: Some(gpt56_reasoning_levels(UltraSupport::No)),
                default_thinking_level: Some(ReasoningEffort::Medium),
                apply_patch_tool_type: Some(ApplyPatchToolType::Freeform),
                tool_overrides: Some(gpt5_overrides.clone()),
                extra_body: Some(gpt56_extra_body()),
                ..Default::default()
            },
        ),
        (
            "gpt-5-4",
            PartialModelInfo {
                display_name: Some("GPT-5.4".into()),
                base_instructions: Some(super::render_instruction_template(GPT_5_4)),
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
                    Capability::OpenAiNativeToolSearch,
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
                base_instructions: Some(super::render_instruction_template(GPT_5_5)),
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
                    Capability::OpenAiNativeToolSearch,
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
                base_instructions: Some(super::render_instruction_template(GPT_5_3_CODEX)),
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
                    Capability::ClientSideToolSearchPromotion,
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

/// Pre-registered GPT-5 model entries shared by both builtin OpenAI
/// providers (`openai` API-key + `openai-chatgpt` OAuth). Empty overrides —
/// model metadata comes from the vendor `models()` catalog above; this map
/// only declares which ids each provider serves so `build_model_registry`
/// emits `(provider, model_id)` pairs and the `/model` picker lists them.
fn openai_gpt5_models() -> BTreeMap<String, PartialProviderModelOverride> {
    // coco's catalog ids use dashes (`gpt-5-3-codex`); the OpenAI backend —
    // both codex (`chatgpt.com/backend-api/codex`) and the platform — expects
    // the DOTTED slug (`gpt-5.3-codex`). Route the wire request through
    // `api_model_name` so the sent id matches what the backend accepts. A
    // dashed id 400s: "The 'gpt-5-3-codex' model is not supported when using
    // Codex with a ChatGPT account."
    let wire = |slug: &str| PartialProviderModelOverride {
        api_model_name: Some(slug.to_string()),
        ..Default::default()
    };
    BTreeMap::from([
        ("gpt-5-6-sol".into(), wire("gpt-5.6-sol")),
        ("gpt-5-6-terra".into(), wire("gpt-5.6-terra")),
        ("gpt-5-6-luna".into(), wire("gpt-5.6-luna")),
        ("gpt-5-4".into(), wire("gpt-5.4")),
        ("gpt-5-5".into(), wire("gpt-5.5")),
        // The ChatGPT/Codex backend exposes the codex model as
        // `gpt-5.3-codex-spark` (rate-limited window); bare `gpt-5.3-codex` 400s.
        ("gpt-5-3-codex".into(), wire("gpt-5.3-codex-spark")),
    ])
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

/// Whether a 5.6 tier advertises the `ultra` rung. `sol` and `terra` do;
/// `luna` stops at `max`.
enum UltraSupport {
    Yes,
    No,
}

/// GPT-5.6 reasoning ladder. Unlike the 5.4/5.5 ladder this has no
/// `disable()` rung — the family always reasons — and it extends past
/// `xhigh` into `max` (and `ultra` on the tiers that offer it).
///
/// The ladder is what makes those two rungs safe workspace-wide: a
/// `--effort ultra` aimed at a model that stops at `xhigh` is clamped by
/// `ModelInfo::resolve_thinking_level` nearest-match before it can reach
/// the wire.
fn gpt56_reasoning_levels(ultra: UltraSupport) -> Vec<ThinkingLevel> {
    let mut levels = vec![
        ThinkingLevel::low(),
        ThinkingLevel::medium(),
        ThinkingLevel::high(),
        ThinkingLevel::xhigh(),
        ThinkingLevel::max(),
    ];
    if matches!(ultra, UltraSupport::Yes) {
        levels.push(ThinkingLevel::ultra());
    }
    levels
}

/// Per-call Responses knobs the 5.6 catalog declares. `textVerbosity`
/// rides the Layer-1 escape hatch because it is a plain per-model wire
/// default, not a cross-provider concept worth a `ModelInfo` field.
fn gpt56_extra_body() -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([(
        "textVerbosity".to_string(),
        serde_json::Value::String("low".into()),
    )])
}
