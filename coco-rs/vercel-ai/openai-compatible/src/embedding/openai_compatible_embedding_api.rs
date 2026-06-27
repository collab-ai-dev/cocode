use serde::Deserialize;
use serde::Serialize;

/// Response from an OpenAI-compatible Embeddings API.
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAICompatibleEmbeddingResponse {
    pub data: Vec<OpenAICompatibleEmbeddingData>,
    pub model: Option<String>,
    pub usage: Option<OpenAICompatibleEmbeddingUsage>,
    /// Provider metadata from the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// A single embedding in the response.
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAICompatibleEmbeddingData {
    pub embedding: Vec<f32>,
    pub index: Option<usize>,
}

/// Usage info from the Embeddings API.
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAICompatibleEmbeddingUsage {
    pub prompt_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[cfg(test)]
#[path = "openai_compatible_embedding_api.test.rs"]
mod tests;
