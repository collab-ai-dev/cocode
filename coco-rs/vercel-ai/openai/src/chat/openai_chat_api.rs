use serde::Deserialize;
use serde::Serialize;

use super::convert_chat_usage::OpenAIChatUsage;

/// Response from the Chat Completions API (non-streaming).
#[derive(Debug, Deserialize)]
pub struct OpenAIChatResponse {
    pub id: Option<String>,
    pub model: Option<String>,
    pub created: Option<u64>,
    pub choices: Vec<OpenAIChatChoice>,
    pub usage: Option<OpenAIChatUsage>,
    pub system_fingerprint: Option<String>,
    pub service_tier: Option<String>,
}

/// A single choice in a chat completion response.
#[derive(Debug, Deserialize)]
pub struct OpenAIChatChoice {
    pub index: Option<u32>,
    pub message: OpenAIChatMessage,
    pub finish_reason: Option<String>,
    pub logprobs: Option<OpenAIChatLogprobs>,
}

/// The message content of a chat completion choice.
#[derive(Debug, Deserialize)]
pub struct OpenAIChatMessage {
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<OpenAIChatToolCall>>,
    pub refusal: Option<String>,
    pub annotations: Option<Vec<OpenAIChatAnnotation>>,
}

/// A tool call in a chat completion response.
#[derive(Debug, Deserialize)]
pub struct OpenAIChatToolCall {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    pub function: OpenAIChatFunctionCall,
}

/// Function call details.
#[derive(Debug, Deserialize)]
pub struct OpenAIChatFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Annotation (citation) in a chat message.
#[derive(Debug, Deserialize)]
pub struct OpenAIChatAnnotation {
    #[serde(rename = "type")]
    pub annotation_type: Option<String>,
    pub url_citation: Option<OpenAIUrlCitation>,
}

/// URL citation detail.
#[derive(Debug, Deserialize)]
pub struct OpenAIUrlCitation {
    pub url: Option<String>,
    pub title: Option<String>,
    pub start_index: Option<u64>,
    pub end_index: Option<u64>,
}

/// Logprobs data.
#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAIChatLogprobs {
    pub content: Option<Vec<OpenAIChatLogprobItem>>,
}

/// A single logprob token entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChatLogprobItem {
    pub token: Option<String>,
    pub logprob: Option<f64>,
    pub bytes: Option<Vec<u8>>,
    pub top_logprobs: Option<Vec<OpenAIChatTopLogprob>>,
}

/// A top logprob entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChatTopLogprob {
    pub token: Option<String>,
    pub logprob: Option<f64>,
    pub bytes: Option<Vec<u8>>,
}

// --- Streaming types ---

/// A streaming chunk from the Chat Completions API.
#[derive(Debug, Deserialize)]
pub struct OpenAIChatChunk {
    pub id: Option<String>,
    pub model: Option<String>,
    pub created: Option<u64>,
    pub choices: Option<Vec<OpenAIChatChunkChoice>>,
    pub usage: Option<OpenAIChatUsage>,
    pub system_fingerprint: Option<String>,
    pub service_tier: Option<String>,
}

/// A choice within a streaming chunk.
#[derive(Debug, Deserialize)]
pub struct OpenAIChatChunkChoice {
    pub index: Option<u32>,
    pub delta: Option<OpenAIChatChunkDelta>,
    pub finish_reason: Option<String>,
    pub logprobs: Option<OpenAIChatLogprobs>,
}

/// Delta content within a streaming chunk choice.
#[derive(Debug, Deserialize)]
pub struct OpenAIChatChunkDelta {
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<OpenAIChatChunkToolCall>>,
    pub refusal: Option<String>,
    pub annotations: Option<Vec<OpenAIChatAnnotation>>,
}

/// A partial tool call within a streaming chunk.
#[derive(Debug, Deserialize)]
pub struct OpenAIChatChunkToolCall {
    pub index: u32,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    pub function: Option<OpenAIChatChunkFunctionCall>,
}

/// Partial function call in a streaming chunk.
#[derive(Debug, Deserialize)]
pub struct OpenAIChatChunkFunctionCall {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[cfg(test)]
#[path = "openai_chat_api.test.rs"]
mod tests;
