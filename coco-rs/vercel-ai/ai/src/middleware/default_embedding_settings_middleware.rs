//! Default settings middleware for embedding models.

use std::sync::Arc;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::EmbeddingModelV4CallOptions;

use super::EmbeddingMiddleware;

/// Settings that can be applied as defaults for embedding models.
#[derive(Default, Clone)]
pub struct DefaultEmbeddingSettings {
    /// Headers.
    pub headers: Option<std::collections::HashMap<String, String>>,
}

/// Middleware that applies default settings to embedding model calls.
pub struct DefaultEmbeddingSettingsMiddleware {
    settings: DefaultEmbeddingSettings,
}

impl DefaultEmbeddingSettingsMiddleware {
    /// Create a new default embedding settings middleware.
    pub fn new(settings: DefaultEmbeddingSettings) -> Self {
        Self { settings }
    }
}

#[async_trait::async_trait]
impl EmbeddingMiddleware for DefaultEmbeddingSettingsMiddleware {
    async fn transform_params(
        &self,
        params: EmbeddingModelV4CallOptions,
    ) -> Result<EmbeddingModelV4CallOptions, AISdkError> {
        let mut params = params;

        // Merge headers
        if let Some(ref default_headers) = self.settings.headers {
            let mut headers = params.headers.unwrap_or_default();
            for (key, value) in default_headers {
                headers.entry(key.clone()).or_insert(value.clone());
            }
            params.headers = Some(headers);
        }

        Ok(params)
    }
}

/// Create a default embedding settings middleware.
pub fn default_embedding_settings_middleware(
    settings: DefaultEmbeddingSettings,
) -> Arc<dyn EmbeddingMiddleware> {
    Arc::new(DefaultEmbeddingSettingsMiddleware::new(settings))
}

#[cfg(test)]
#[path = "default_embedding_settings_middleware.test.rs"]
mod tests;
