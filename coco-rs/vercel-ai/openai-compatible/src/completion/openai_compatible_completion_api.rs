use serde::Deserialize;
use serde::Serialize;

/// Response from the legacy Completions API.
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAICompatibleCompletionResponse {
    pub id: Option<String>,
    pub model: Option<String>,
    pub created: Option<u64>,
    pub choices: Vec<OpenAICompatibleCompletionChoice>,
    pub usage: Option<OpenAICompatibleCompletionUsage>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAICompatibleCompletionChoice {
    pub text: Option<String>,
    pub index: Option<u32>,
    pub finish_reason: Option<String>,
    pub logprobs: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAICompatibleCompletionUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

/// A streaming chunk from the Completions API.
#[derive(Debug, Deserialize)]
pub struct OpenAICompatibleCompletionChunk {
    pub id: Option<String>,
    pub model: Option<String>,
    pub created: Option<u64>,
    pub choices: Option<Vec<OpenAICompatibleCompletionChunkChoice>>,
    pub usage: Option<OpenAICompatibleCompletionUsage>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAICompatibleCompletionChunkChoice {
    pub text: Option<String>,
    pub index: Option<u32>,
    pub finish_reason: Option<String>,
}

#[cfg(test)]
#[path = "openai_compatible_completion_api.test.rs"]
mod tests;
