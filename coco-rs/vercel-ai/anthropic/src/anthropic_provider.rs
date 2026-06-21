use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::ProviderV4;
use vercel_ai_provider::errors::LoadAPIKeyError;
use vercel_ai_provider::errors::NoSuchModelError;
use vercel_ai_provider::provider::v4::FromEnvProvider;
use vercel_ai_provider_utils::load_api_key;

use crate::anthropic_config::AdapterAccountKind;
use crate::anthropic_config::AnthropicConfig;
use crate::anthropic_config::AnthropicModelCapabilities;
use crate::anthropic_config::ProviderTopology;
use crate::messages::AnthropicMessagesLanguageModel;

/// Settings for creating an Anthropic provider.
pub struct AnthropicProviderSettings {
    /// Base URL (default: "https://api.anthropic.com/v1").
    pub base_url: Option<String>,
    /// API key (sent via `x-api-key` header). Falls back to `ANTHROPIC_API_KEY` env var.
    /// Mutually exclusive with `auth_token`.
    pub api_key: Option<String>,
    /// Auth token (sent via `Authorization: Bearer` header). Falls back to `ANTHROPIC_AUTH_TOKEN` env var.
    /// Mutually exclusive with `api_key`.
    pub auth_token: Option<String>,
    /// Custom headers to include in every request.
    pub headers: Option<HashMap<String, String>>,
    /// Provider name (default: "anthropic.messages").
    pub name: Option<String>,
    /// Shared HTTP client.
    pub client: Option<Arc<reqwest::Client>>,
    /// When false, the model will use JSON tool fallback for structured outputs.
    /// Defaults to true.
    pub supports_native_structured_output: Option<bool>,
    /// When false, `strict` on tool definitions will be ignored and a warning emitted.
    /// Defaults to true.
    pub supports_strict_tools: Option<bool>,
    /// When `true`, `base_url` is the complete endpoint URL — no API path
    /// suffix is appended. Default (`None`): auto-detect duplicate suffixes.
    pub full_url: Option<bool>,

    // ─── Prompt-cache + beta-policy fields (design §10.0) ─────────────
    /// Resolved per-model capabilities. Empty = unknown-model safe default.
    pub capabilities: AnthropicModelCapabilities,
    /// Endpoint topology — drives first-party-only beta inclusion.
    pub provider_topology: ProviderTopology,
    /// Enable experimental betas. Default true.
    pub experimental_betas_enabled: bool,
    /// Disable interleaved thinking even on capable models.
    pub disable_interleaved_thinking: bool,
    /// Show thinking summaries (suppresses redact-thinking beta when true).
    pub show_thinking_summaries: bool,
    /// Non-interactive session flag (suppresses redact-thinking beta).
    pub non_interactive: bool,
    /// 1h-TTL allowlist patterns. Each entry is either an exact match for
    /// `query_source`, or a `prefix*` glob.
    pub prompt_cache_allowlist: Vec<String>,
    /// Session-stable account / billing identity (R3-F3).
    pub account_kind: AdapterAccountKind,
    /// Session-stable subscriber overage flag (R3-F3).
    pub in_overage: bool,
}

impl Default for AnthropicProviderSettings {
    fn default() -> Self {
        Self {
            base_url: None,
            api_key: None,
            auth_token: None,
            headers: None,
            name: None,
            client: None,
            supports_native_structured_output: None,
            supports_strict_tools: None,
            full_url: None,
            capabilities: AnthropicModelCapabilities::default(),
            provider_topology: ProviderTopology::default(),
            experimental_betas_enabled: true,
            disable_interleaved_thinking: false,
            show_thinking_summaries: false,
            non_interactive: false,
            prompt_cache_allowlist: Vec::new(),
            account_kind: AdapterAccountKind::default(),
            in_overage: false,
        }
    }
}

/// Anthropic multi-model provider.
///
/// Implements `ProviderV4` and provides access to Messages language models.
/// Anthropic does not offer embedding or image generation models.
pub struct AnthropicProvider {
    provider_name: String,
    base_url: String,
    headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    client: Option<Arc<reqwest::Client>>,
    supports_native_structured_output: Option<bool>,
    supports_strict_tools: Option<bool>,
    full_url: Option<bool>,

    // ─── Prompt-cache + beta-policy fields (design §10.0) ─────────────
    capabilities: AnthropicModelCapabilities,
    provider_topology: ProviderTopology,
    experimental_betas_enabled: bool,
    disable_interleaved_thinking: bool,
    show_thinking_summaries: bool,
    non_interactive: bool,
    prompt_cache_allowlist: Vec<String>,
    account_kind: AdapterAccountKind,
    in_overage: bool,
}

impl AnthropicProvider {
    /// Create a new provider from settings.
    pub fn new(settings: AnthropicProviderSettings) -> Self {
        let provider_name = settings.name.unwrap_or_else(|| "anthropic.messages".into());
        let base_url = settings
            .base_url
            .or_else(|| std::env::var("ANTHROPIC_BASE_URL").ok())
            .unwrap_or_else(|| "https://api.anthropic.com/v1".into())
            .trim_end_matches('/')
            .to_string();

        // Validate mutual exclusivity
        if settings.api_key.is_some() && settings.auth_token.is_some() {
            tracing::warn!(
                "Both api_key and auth_token were provided. Please use only one authentication method."
            );
        }

        let api_key = settings.api_key;
        let auth_token = settings.auth_token;
        let custom_headers = settings.headers.unwrap_or_default();

        let headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync> = Arc::new(move || {
            let mut h = HashMap::new();

            // Always include anthropic-version
            h.insert("anthropic-version".into(), "2023-06-01".into());

            // Auth: auth_token → Bearer, otherwise x-api-key
            if let Some(ref token) = auth_token {
                h.insert("Authorization".into(), format!("Bearer {token}"));
            } else {
                let key = load_api_key(api_key.as_deref(), "ANTHROPIC_API_KEY", "Anthropic")
                    .unwrap_or_default();
                if !key.is_empty() {
                    h.insert("x-api-key".into(), key);
                }
            }

            // Merge custom headers (custom overrides defaults)
            for (k, v) in &custom_headers {
                h.insert(k.clone(), v.clone());
            }

            h
        });

        Self {
            provider_name,
            base_url,
            headers,
            client: settings.client,
            supports_native_structured_output: settings.supports_native_structured_output,
            supports_strict_tools: settings.supports_strict_tools,
            full_url: settings.full_url,
            capabilities: settings.capabilities,
            provider_topology: settings.provider_topology,
            experimental_betas_enabled: settings.experimental_betas_enabled,
            disable_interleaved_thinking: settings.disable_interleaved_thinking,
            show_thinking_summaries: settings.show_thinking_summaries,
            non_interactive: settings.non_interactive,
            prompt_cache_allowlist: settings.prompt_cache_allowlist,
            account_kind: settings.account_kind,
            in_overage: settings.in_overage,
        }
    }

    fn make_config(&self) -> Arc<AnthropicConfig> {
        Arc::new(AnthropicConfig {
            provider: self.provider_name.clone(),
            base_url: self.base_url.clone(),
            headers: self.headers.clone(),
            client: self.client.clone(),
            supports_native_structured_output: self.supports_native_structured_output,
            supports_strict_tools: self.supports_strict_tools,
            full_url: self.full_url,
            capabilities: self.capabilities,
            provider_topology: self.provider_topology,
            experimental_betas_enabled: self.experimental_betas_enabled,
            disable_interleaved_thinking: self.disable_interleaved_thinking,
            show_thinking_summaries: self.show_thinking_summaries,
            non_interactive: self.non_interactive,
            prompt_cache_allowlist: self.prompt_cache_allowlist.clone(),
            account_kind: self.account_kind,
            in_overage: self.in_overage,
        })
    }

    /// Get a Messages API language model.
    pub fn messages(&self, model_id: &str) -> AnthropicMessagesLanguageModel {
        AnthropicMessagesLanguageModel::new(model_id, self.make_config())
    }

    /// Alias for `messages()`.
    pub fn chat(&self, model_id: &str) -> AnthropicMessagesLanguageModel {
        self.messages(model_id)
    }
}

#[async_trait]
impl ProviderV4 for AnthropicProvider {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    fn language_model(&self, model_id: &str) -> Result<Arc<dyn LanguageModelV4>, NoSuchModelError> {
        Ok(Arc::new(self.messages(model_id)))
    }

    fn embedding_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn vercel_ai_provider::EmbeddingModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model_with_type(
            model_id,
            "embeddingModel",
        ))
    }

    fn image_model(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn vercel_ai_provider::ImageModelV4>, NoSuchModelError> {
        Err(NoSuchModelError::for_model_with_type(
            model_id,
            "imageModel",
        ))
    }
}

impl FromEnvProvider for AnthropicProvider {
    fn from_env() -> Result<Self, LoadAPIKeyError> {
        // Validate that a key exists
        load_api_key(None, "ANTHROPIC_API_KEY", "Anthropic")?;
        Ok(Self::new(AnthropicProviderSettings::default()))
    }
}

/// Create an Anthropic provider with custom settings.
pub fn create_anthropic(settings: AnthropicProviderSettings) -> AnthropicProvider {
    AnthropicProvider::new(settings)
}

/// Create a default Anthropic provider using env vars.
pub fn anthropic() -> AnthropicProvider {
    AnthropicProvider::new(AnthropicProviderSettings::default())
}

#[cfg(test)]
#[path = "anthropic_provider.test.rs"]
mod tests;
