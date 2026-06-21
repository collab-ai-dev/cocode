use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider_utils::ResponseHandler;

use crate::metadata_extractor::MetadataExtractor;
use crate::provider_options::PromptTokensTotalSemantics;
use crate::provider_profile::OpenAICompatibleProviderProfile;

/// Settings for creating an OpenAI-compatible provider.
#[derive(Default)]
pub struct OpenAICompatibleProviderSettings {
    /// Base URL for the API.
    pub base_url: Option<String>,
    /// API key. Falls back to the env var specified by `api_key_env_var`.
    pub api_key: Option<String>,
    /// Environment variable name for the API key (e.g., "XAI_API_KEY").
    pub api_key_env_var: Option<String>,
    /// Human-readable description of the API key source (e.g., "xAI").
    pub api_key_description: Option<String>,
    /// Custom headers to include in every request.
    pub headers: Option<HashMap<String, String>>,
    /// Query parameters to append to every request URL.
    pub query_params: Option<HashMap<String, String>>,
    /// Provider name (e.g., "xai", "groq").
    pub name: Option<String>,
    /// Shared HTTP client.
    pub client: Option<Arc<reqwest::Client>>,
    /// Whether to include usage in streaming responses. Defaults to `true`.
    pub include_usage: Option<bool>,
    /// Whether the provider supports structured outputs (json_schema). Defaults to `false`.
    pub supports_structured_outputs: Option<bool>,
    /// Optional request body transform applied before sending.
    pub transform_request_body: Option<Arc<dyn Fn(Value) -> Value + Send + Sync>>,
    /// Optional metadata extractor for provider-specific response metadata.
    pub metadata_extractor: Option<Arc<dyn MetadataExtractor>>,
    /// Optional custom error handler for failed API responses.
    /// If not set, uses the default OpenAI-compatible error handler.
    pub error_handler: Option<Arc<dyn ResponseHandler<AISdkError>>>,
    /// Whether `prompt_tokens` already includes cached tokens. Defaults
    /// to `Inclusive` (the OpenAI convention).
    pub prompt_tokens_total_semantics: PromptTokensTotalSemantics,
    /// Provider-specific wire behavior. Defaults to generic OpenAI-compatible.
    pub provider_profile: Option<OpenAICompatibleProviderProfile>,
    /// When `true`, `base_url` is the complete endpoint URL — no API path
    /// suffix is appended. Default (`None`): auto-detect duplicate suffixes.
    pub full_url: Option<bool>,
}
