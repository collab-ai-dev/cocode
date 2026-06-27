use serde::Deserialize;
use serde::Serialize;

/// Response from the OpenAI Embeddings API.
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAIEmbeddingResponse {
    pub data: Vec<OpenAIEmbeddingData>,
    pub model: Option<String>,
    pub usage: Option<OpenAIEmbeddingUsage>,
}

/// A single embedding in the response.
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAIEmbeddingData {
    pub embedding: Vec<f32>,
    pub index: Option<usize>,
}

/// Usage info from the Embeddings API.
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAIEmbeddingUsage {
    pub prompt_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[cfg(test)]
#[path = "openai_embedding_api.test.rs"]
mod tests;
