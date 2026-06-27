use serde::Deserialize;
use std::collections::HashMap;

/// Provider-specific options for OpenAI completion models.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAICompletionProviderOptions {
    pub echo: Option<bool>,
    pub logit_bias: Option<HashMap<String, f64>>,
    /// `true` for default logprobs, or a number for top N.
    pub logprobs: Option<serde_json::Value>,
    pub suffix: Option<String>,
    pub user: Option<String>,
}

/// Extract completion-specific options from provider options.
pub fn extract_completion_options(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
) -> OpenAICompletionProviderOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("openai"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<OpenAICompletionProviderOptions>(v).ok())
        .unwrap_or_default()
}
