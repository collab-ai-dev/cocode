use serde::Deserialize;
use vercel_ai_provider::ProviderOptions;

/// Provider-specific options for the xAI Responses API.
///
/// Mirrors `xaiLanguageModelResponsesOptions` from
/// `xai-responses-language-model-options.ts`, extracted from
/// `provider_options["xai"]`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct XaiResponsesProviderOptions {
    /// Constrains reasoning effort: `none` | `low` | `medium` | `high`.
    /// Passed through verbatim to the `reasoning.effort` field.
    #[serde(rename = "reasoningEffort")]
    pub reasoning_effort: Option<String>,
    /// Reasoning summary verbosity: `auto` | `concise` | `detailed`.
    /// Passed through to the `reasoning.summary` field.
    #[serde(rename = "reasoningSummary")]
    pub reasoning_summary: Option<String>,
    /// Whether to return log probabilities of the output tokens.
    pub logprobs: Option<bool>,
    /// Number of most likely tokens to return per position (0..=8). Implies
    /// `logprobs`.
    #[serde(rename = "topLogprobs")]
    pub top_logprobs: Option<i64>,
    /// Whether to store the request/response for later retrieval. Defaults to
    /// `true` on the server. Must be `false` for Zero Data Retention teams.
    pub store: Option<bool>,
    /// ID of a previous response to continue from.
    #[serde(rename = "previousResponseId")]
    pub previous_response_id: Option<String>,
    /// Additional output data to include (e.g. `file_search_call.results`).
    pub include: Option<Vec<String>>,
}

/// Extract xAI Responses options from the generic provider-options map (the
/// `"xai"` namespace). Returns defaults when absent or malformed.
pub fn extract_xai_responses_options(
    provider_options: &Option<ProviderOptions>,
) -> XaiResponsesProviderOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("xai"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<XaiResponsesProviderOptions>(v).ok())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "xai_responses_options.test.rs"]
mod tests;
