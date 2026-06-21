//! Default settings middleware for language models.

use std::sync::Arc;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Middleware;
use vercel_ai_provider::language_model_middleware::TransformParamsOptions;
use vercel_ai_provider::shared::ProviderOptions;

use crate::util::merge_objects;

/// Settings that can be applied as defaults.
#[derive(Default, Clone)]
pub struct DefaultSettings {
    /// Maximum output tokens.
    pub max_output_tokens: Option<u64>,
    /// Temperature for sampling.
    pub temperature: Option<f32>,
    /// Stop sequences.
    pub stop_sequences: Option<Vec<String>>,
    /// Top-p sampling.
    pub top_p: Option<f32>,
    /// Top-k sampling.
    pub top_k: Option<u64>,
    /// Presence penalty.
    pub presence_penalty: Option<f32>,
    /// Frequency penalty.
    pub frequency_penalty: Option<f32>,
    /// Response format.
    pub response_format: Option<vercel_ai_provider::ResponseFormat>,
    /// Random seed.
    pub seed: Option<u64>,
    /// Tool choice.
    pub tool_choice: Option<vercel_ai_provider::LanguageModelV4ToolChoice>,
    /// Headers.
    pub headers: Option<std::collections::HashMap<String, String>>,
    /// Provider options.
    pub provider_options: Option<ProviderOptions>,
}

/// Middleware that applies default settings to language model calls.
///
/// Settings are merged with the call parameters, with the call parameters
/// taking precedence over the defaults.
pub struct DefaultSettingsMiddleware {
    settings: DefaultSettings,
}

impl DefaultSettingsMiddleware {
    /// Create a new default settings middleware.
    pub fn new(settings: DefaultSettings) -> Self {
        Self { settings }
    }
}

#[async_trait::async_trait]
impl LanguageModelV4Middleware for DefaultSettingsMiddleware {
    async fn transform_params(
        &self,
        options: TransformParamsOptions,
    ) -> Result<LanguageModelV4CallOptions, AISdkError> {
        let mut params = options.params;

        // Apply defaults only if not already set
        if params.max_output_tokens.is_none() && self.settings.max_output_tokens.is_some() {
            params.max_output_tokens = self.settings.max_output_tokens;
        }
        if params.temperature.is_none() && self.settings.temperature.is_some() {
            params.temperature = self.settings.temperature;
        }
        if params.stop_sequences.is_none() && self.settings.stop_sequences.is_some() {
            params.stop_sequences = self.settings.stop_sequences.clone();
        }
        if params.top_p.is_none() && self.settings.top_p.is_some() {
            params.top_p = self.settings.top_p;
        }
        if params.top_k.is_none() && self.settings.top_k.is_some() {
            params.top_k = self.settings.top_k;
        }
        if params.presence_penalty.is_none() && self.settings.presence_penalty.is_some() {
            params.presence_penalty = self.settings.presence_penalty;
        }
        if params.frequency_penalty.is_none() && self.settings.frequency_penalty.is_some() {
            params.frequency_penalty = self.settings.frequency_penalty;
        }
        if params.response_format.is_none() && self.settings.response_format.is_some() {
            params.response_format = self.settings.response_format.clone();
        }
        if params.seed.is_none() && self.settings.seed.is_some() {
            params.seed = self.settings.seed;
        }
        if params.tool_choice.is_none() && self.settings.tool_choice.is_some() {
            params.tool_choice = self.settings.tool_choice.clone();
        }

        // Merge headers
        if let Some(ref default_headers) = self.settings.headers {
            let mut headers = params.headers.unwrap_or_default();
            for (key, value) in default_headers {
                headers.entry(key.clone()).or_insert(value.clone());
            }
            params.headers = Some(headers);
        }

        // Merge provider options
        if self.settings.provider_options.is_some() || params.provider_options.is_some() {
            // Convert ProviderOptions to Value for merging
            let default_value = self
                .settings
                .provider_options
                .as_ref()
                .and_then(|po| serde_json::to_value(po).ok());
            let param_value = params
                .provider_options
                .as_ref()
                .and_then(|po| serde_json::to_value(po).ok());

            if let Some(merged) = merge_objects(default_value, param_value)
                && let Ok(po) = serde_json::from_value(merged)
            {
                params.provider_options = Some(po);
            }
        }

        Ok(params)
    }
}

/// Create a default settings middleware.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::middleware::{default_settings_middleware, DefaultSettings};
///
/// let middleware = default_settings_middleware(DefaultSettings {
///     temperature: Some(0.7),
///     max_output_tokens: Some(1000),
///     ..Default::default()
/// });
/// ```
pub fn default_settings_middleware(
    settings: DefaultSettings,
) -> Arc<dyn LanguageModelV4Middleware> {
    Arc::new(DefaultSettingsMiddleware::new(settings))
}

#[cfg(test)]
#[path = "default_settings_middleware.test.rs"]
mod tests;
