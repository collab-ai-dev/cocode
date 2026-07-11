//! xAI (Grok) vendor catalog — the `xai` provider only. Users declare Grok
//! models against this provider via `config home/providers.json` or
//! `config home/models.json`.
//!
//! `api: OpenaiCompat` with the canonical instance name `xai` is what routes
//! model construction to the dedicated `vercel-ai-xai` crate in
//! `services/inference::model_factory::build_xai` (the same name-dispatch
//! pattern as `deepseek-openai`). The instance name `xai` also makes the
//! runtime wrap `provider_options` under the `"xai"` namespace the crate reads.

use coco_types::ProviderApi;

use crate::model::partial::PartialModelInfo;
use crate::provider::PartialProviderConfig;

/// Canonical name of the builtin xAI provider instance. Shared by the builtin
/// registry entry here and the model-factory dispatch check so a rename is one
/// compiler-enforced edit, not two drifting literals.
pub const XAI_PROVIDER: &str = "xai";

pub(super) fn providers() -> Vec<(&'static str, PartialProviderConfig)> {
    vec![(
        XAI_PROVIDER,
        PartialProviderConfig {
            api: Some(ProviderApi::OpenaiCompat),
            env_key: Some("XAI_API_KEY".into()),
            // OpenAI-compatible endpoint — the SDK appends `/chat/completions`.
            base_url: Some("https://api.x.ai/v1".into()),
            ..Default::default()
        },
    )]
}

pub(super) fn models() -> Vec<(&'static str, PartialModelInfo)> {
    Vec::new()
}
