use serde::Deserialize;
use serde::Serialize;

/// Response from the legacy Completions API.
#[derive(Debug, Deserialize)]
pub struct OpenAICompletionResponse {
    pub id: Option<String>,
    pub model: Option<String>,
    pub created: Option<u64>,
    pub choices: Vec<OpenAICompletionChoice>,
    pub usage: Option<OpenAICompletionUsage>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAICompletionChoice {
    pub text: Option<String>,
    pub index: Option<u32>,
    pub finish_reason: Option<String>,
    pub logprobs: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAICompletionUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

/// A streaming chunk from the Completions API.
#[derive(Debug, Deserialize)]
pub struct OpenAICompletionChunk {
    pub id: Option<String>,
    pub model: Option<String>,
    pub created: Option<u64>,
    pub choices: Option<Vec<OpenAICompletionChunkChoice>>,
    pub usage: Option<OpenAICompletionUsage>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAICompletionChunkChoice {
    pub text: Option<String>,
    pub index: Option<u32>,
    pub finish_reason: Option<String>,
    pub logprobs: Option<serde_json::Value>,
}

#[cfg(test)]
#[path = "openai_completion_api.test.rs"]
mod tests;
