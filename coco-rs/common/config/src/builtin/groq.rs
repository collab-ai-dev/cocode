//! Groq vendor catalog — the `groq` provider only. Users declare Groq models
//! against this provider via `config home/providers.json` or
//! `config home/models.json`.
//!
//! `api: OpenaiCompat` with the canonical instance name `groq` routes model
//! construction to the dedicated `vercel-ai-groq` crate in
//! `services/inference::model_factory::build_groq`. The instance name `groq`
//! also makes the runtime wrap `provider_options` under the `"groq"` namespace
//! the crate reads.

use coco_types::ProviderApi;

use crate::model::partial::PartialModelInfo;
use crate::provider::PartialProviderConfig;

/// Canonical name of the builtin Groq provider instance. Shared by the builtin
/// registry entry here and the model-factory dispatch check so the name that
/// selects the dedicated `vercel-ai-groq` crate cannot drift.
pub const GROQ_PROVIDER: &str = "groq";

pub(super) fn providers() -> Vec<(&'static str, PartialProviderConfig)> {
    vec![(
        GROQ_PROVIDER,
        PartialProviderConfig {
            api: Some(ProviderApi::OpenaiCompat),
            env_key: Some("GROQ_API_KEY".into()),
            // OpenAI-compatible endpoint — the SDK appends `/chat/completions`.
            base_url: Some("https://api.groq.com/openai/v1".into()),
            ..Default::default()
        },
    )]
}

pub(super) fn models() -> Vec<(&'static str, PartialModelInfo)> {
    Vec::new()
}
