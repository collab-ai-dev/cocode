use serde::Deserialize;
use vercel_ai_provider::ProviderOptions;

/// Provider-specific options for xAI chat models.
///
/// Mirrors `xaiLanguageModelChatOptions` from
/// `xai-chat-language-model-options.ts`, extracted from
/// `provider_options["xai"]`.
///
/// The upstream `searchParameters` (Live Search) option is intentionally NOT
/// ported: xAI deprecated it in favor of the Agent Tools API and the endpoint
/// now rejects requests that carry it with a "Live search is deprecated"
/// error, so it is dead on the wire.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct XaiChatProviderOptions {
    /// Constrains reasoning effort: `none` | `low` | `medium` | `high`.
    /// Passed through verbatim; not every Grok model accepts every value.
    #[serde(rename = "reasoningEffort")]
    pub reasoning_effort: Option<String>,
    /// Whether to return log probabilities of the output tokens.
    pub logprobs: Option<bool>,
    /// Number of most likely tokens to return per position (0..=8). Implies
    /// `logprobs`.
    #[serde(rename = "topLogprobs")]
    pub top_logprobs: Option<i64>,
    /// Whether to enable parallel function calling during tool use.
    pub parallel_function_calling: Option<bool>,
}

/// Extract xAI chat options from the generic provider-options map (the `"xai"`
/// namespace). Returns defaults when absent or malformed.
pub fn extract_xai_chat_options(
    provider_options: &Option<ProviderOptions>,
) -> XaiChatProviderOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("xai"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<XaiChatProviderOptions>(v).ok())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "xai_chat_options.test.rs"]
mod tests;
