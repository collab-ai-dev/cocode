use serde::Deserialize;
use std::collections::BTreeMap;
use vercel_ai_provider_utils::ExtractExtras;
use vercel_ai_provider_utils::extract_namespaced;

use crate::chat::openai_chat_options::PromptCacheRetention;
use crate::chat::openai_chat_options::ReasoningEffort;
use crate::chat::openai_chat_options::ServiceTier;
use crate::chat::openai_chat_options::TextVerbosity;
use crate::openai_capabilities::SystemMessageMode;

/// A context management entry for server-side compaction.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextManagementEntry {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub compact_threshold: f64,
}

/// Provider-specific options for the OpenAI Responses API.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIResponsesProviderOptions {
    pub context_management: Option<Vec<ContextManagementEntry>>,
    pub conversation: Option<String>,
    pub include: Option<Vec<String>>,
    pub instructions: Option<String>,
    pub logprobs: Option<serde_json::Value>,
    pub max_tool_calls: Option<u64>,
    pub metadata: Option<serde_json::Value>,
    pub parallel_tool_calls: Option<bool>,
    pub previous_response_id: Option<String>,
    pub prompt_cache_key: Option<String>,
    pub prompt_cache_retention: Option<PromptCacheRetention>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub reasoning_summary: Option<String>,
    pub safety_identifier: Option<String>,
    pub service_tier: Option<ServiceTier>,
    pub store: Option<bool>,
    pub strict_json_schema: Option<bool>,
    pub text_verbosity: Option<TextVerbosity>,
    pub truncation: Option<String>,
    pub user: Option<String>,
    pub system_message_mode: Option<SystemMessageMode>,
    pub force_reasoning: Option<bool>,

    // See `OpenAIChatProviderOptions::extra` for rationale. Typed-consumed
    // keys never leak to the wire body root; only genuine extra_body
    // fields flow through.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ExtractExtras for OpenAIResponsesProviderOptions {
    fn take_extras(&mut self) -> BTreeMap<String, serde_json::Value> {
        std::mem::take(&mut self.extra)
    }
}

/// Extract OpenAI Responses-specific options from the generic
/// provider options map. Single-namespace `"openai"`.
pub fn extract_responses_options(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
) -> (
    OpenAIResponsesProviderOptions,
    BTreeMap<String, serde_json::Value>,
) {
    extract_namespaced(provider_options.as_ref(), "openai", "openai")
}

#[cfg(test)]
#[path = "openai_responses_options.test.rs"]
mod tests;
