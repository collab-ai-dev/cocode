use serde::Deserialize;
use vercel_ai_provider::ProviderOptions;

/// Provider-specific options for Groq chat models.
///
/// Mirrors `groqLanguageModelChatOptions` from
/// `groq-chat-language-model-options.ts`. Extracted from
/// `provider_options["groq"]`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroqChatProviderOptions {
    /// Reasoning output format: `parsed` | `raw` | `hidden`.
    pub reasoning_format: Option<String>,
    /// Reasoning effort: `none` | `default` | `low` | `medium` | `high`.
    pub reasoning_effort: Option<String>,
    /// Whether to enable parallel function calling during tool use.
    pub parallel_tool_calls: Option<bool>,
    /// A unique identifier representing the end-user.
    pub user: Option<String>,
    /// Whether to use structured outputs. Defaults to `true`.
    pub structured_outputs: Option<bool>,
    /// Whether to use strict JSON schema validation. Defaults to `true`.
    pub strict_json_schema: Option<bool>,
    /// Service tier: `on_demand` | `performance` | `flex` | `auto`.
    pub service_tier: Option<String>,
}

/// Extract Groq chat options from the generic provider-options map
/// (the `"groq"` namespace). Returns defaults when absent or malformed.
pub fn extract_groq_chat_options(
    provider_options: &Option<ProviderOptions>,
) -> GroqChatProviderOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("groq"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<GroqChatProviderOptions>(v).ok())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "groq_chat_options.test.rs"]
mod tests;
