use std::collections::HashMap;
use std::sync::Arc;

use regex::Regex;
use serde_json::Value;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider_utils::ResponseHandler;

use crate::metadata_extractor::MetadataExtractor;
use crate::provider_options::PromptTokensTotalSemantics;
use crate::provider_profile::OpenAICompatibleProviderProfile;

/// Callback that returns supported URL patterns by MIME type.
pub type SupportedUrlsFn = dyn Fn() -> HashMap<String, Vec<Regex>> + Send + Sync;

/// Shared configuration passed to each OpenAI-compatible model instance.
pub struct OpenAICompatibleConfig {
    /// Provider identifier (e.g., "xai.chat", "groq.chat").
    pub provider: String,
    /// Base URL for the API (e.g., "https://api.x.ai/v1").
    pub base_url: String,
    /// Lazy header supplier — called per-request to get auth + custom headers.
    pub headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    /// Optional query parameters appended to every request URL.
    pub query_params: Option<HashMap<String, String>>,
    /// Optional shared HTTP client for connection pooling.
    pub client: Option<Arc<reqwest::Client>>,
    /// Whether to include usage in streaming responses (`stream_options: { include_usage: true }`).
    pub include_usage: bool,
    /// Whether the provider supports structured outputs (json_schema response format).
    pub supports_structured_outputs: bool,
    /// Optional request body transform applied before sending.
    pub transform_request_body: Option<Arc<dyn Fn(Value) -> Value + Send + Sync>>,
    /// Optional metadata extractor for provider-specific response metadata.
    pub metadata_extractor: Option<Arc<dyn MetadataExtractor>>,
    /// Optional supported URL patterns by MIME type.
    /// If `None`, defaults to empty (no URL support). TS default is also empty `{}`.
    pub supported_urls: Option<Arc<SupportedUrlsFn>>,
    /// Error response handler for failed API calls.
    pub error_handler: Arc<dyn ResponseHandler<AISdkError>>,
    /// Whether prompt token totals include cached tokens.
    pub prompt_tokens_total_semantics: PromptTokensTotalSemantics,
    /// Provider-specific wire behavior.
    pub provider_profile: OpenAICompatibleProviderProfile,
    /// When `true`, `base_url` is the complete endpoint URL — no API path
    /// suffix is appended. Default (`None`): auto-detect duplicate suffixes.
    pub full_url: Option<bool>,
}

impl OpenAICompatibleConfig {
    /// Build a full URL from a path segment (e.g., "/chat/completions"),
    /// appending any configured query parameters.
    ///
    /// If `full_url` is set, or `base_url` already ends with the path,
    /// returns `base_url` as-is to avoid duplication.
    pub fn url(&self, path: &str) -> String {
        let base = if self.full_url.unwrap_or(false) || self.base_url.ends_with(path) {
            self.base_url.clone()
        } else {
            format!("{}{path}", self.base_url)
        };
        match &self.query_params {
            Some(params) if !params.is_empty() => {
                let query: String = form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(params.iter())
                    .finish();
                format!("{base}?{query}")
            }
            _ => base,
        }
    }

    /// Get the current headers by invoking the lazy supplier.
    pub fn get_headers(&self) -> HashMap<String, String> {
        (self.headers)()
    }

    /// Apply the optional request body transform.
    pub fn transform_body(&self, body: Value) -> Value {
        match &self.transform_request_body {
            Some(transform) => transform(body),
            None => body,
        }
    }

    /// Get the provider options key (first segment before '.').
    ///
    /// E.g., `"xai.chat"` → `"xai"`, `"groq"` → `"groq"`.
    pub fn provider_options_name(&self) -> &str {
        self.provider.split('.').next().unwrap_or(&self.provider)
    }
}
