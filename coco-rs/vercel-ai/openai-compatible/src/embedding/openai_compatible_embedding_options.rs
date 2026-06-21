use serde::Deserialize;

/// Provider-specific options for OpenAI-compatible embedding models.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAICompatibleEmbeddingProviderOptions {
    pub dimensions: Option<usize>,
    pub user: Option<String>,
}

/// Extract embedding-specific options from provider options.
pub fn extract_embedding_options(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
    provider_name: &str,
) -> OpenAICompatibleEmbeddingProviderOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get(provider_name))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<OpenAICompatibleEmbeddingProviderOptions>(v).ok())
        .unwrap_or_default()
}
