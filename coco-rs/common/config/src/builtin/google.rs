//! Google vendor catalog — `google` provider + Gemini models.

use coco_types::Capability;
use coco_types::OAuthFlowId;
use coco_types::ProviderApi;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;

use crate::model::partial::PartialModelInfo;
use crate::positive::PositiveTokens;
use crate::provider::PartialProviderConfig;
use crate::provider::ProviderAuth;

const GEMINI: &str = include_str!("../../instructions/gemini_prompt.md");

pub(super) fn providers() -> Vec<(&'static str, PartialProviderConfig)> {
    vec![
        (
            "google",
            PartialProviderConfig {
                api: Some(ProviderApi::Gemini),
                env_key: Some("GOOGLE_API_KEY".into()),
                // **Must end with `/v1beta`.** Same reason as Anthropic — the
                // SDK appends `/models/<id>:generateContent` to `base_url`
                // without auto-detecting missing version segments.
                base_url: Some("https://generativelanguage.googleapis.com/v1beta".into()),
                ..Default::default()
            },
        ),
        (
            // Gemini Code Assist subscription: authenticated by
            // `coco login gemini` (Google OAuth) and served by the
            // `vercel-ai-google-codeassist` transport (Bearer + `{project,
            // request}` envelope + `:method` RPC + lazy project onboarding).
            // `model_factory::build_google` routes `auth: OAuth` there.
            super::GEMINI_CODE_ASSIST_PROVIDER,
            PartialProviderConfig {
                api: Some(ProviderApi::Gemini),
                auth: Some(ProviderAuth::OAuth {
                    flow: OAuthFlowId::GeminiCodeAssist,
                }),
                base_url: Some("https://cloudcode-pa.googleapis.com/v1internal".into()),
                ..Default::default()
            },
        ),
    ]
}

pub(super) fn models() -> Vec<(&'static str, PartialModelInfo)> {
    vec![(
        "gemini-3.1-pro-preview",
        PartialModelInfo {
            display_name: Some("Gemini 3.1 Pro Preview".into()),
            base_instructions: Some(GEMINI.into()),
            context_window: Some(PositiveTokens::new(1_000_000)),
            max_output_tokens: Some(PositiveTokens::new(65_536)),
            capabilities: Some(vec![
                Capability::TextGeneration,
                Capability::Streaming,
                Capability::ToolCalling,
                Capability::Vision,
                // Gemini natively supports `responseMimeType:
                // "application/json"` + `responseSchema`. The
                // `vercel-ai-google` adapter emits both when
                // `response_format: ResponseFormat::Json` is set.
                Capability::StructuredOutput,
                Capability::ExtendedThinking,
                Capability::AdaptiveThinking,
                Capability::ParallelToolCalls,
                Capability::ClientSideToolSearch,
            ]),
            supported_thinking_levels: Some(gemini_thinking_levels()),
            default_thinking_level: Some(ReasoningEffort::Medium),
            ..Default::default()
        },
    )]
}

fn gemini_thinking_levels() -> Vec<ThinkingLevel> {
    vec![
        ThinkingLevel::low(),
        ThinkingLevel::medium(),
        ThinkingLevel::high(),
    ]
}
