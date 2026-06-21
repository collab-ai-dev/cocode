use serde::Deserialize;
use serde::Serialize;

use super::convert_chat_usage::OpenAICompatibleChatUsage;

/// Response from the Chat Completions API (non-streaming).
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAICompatibleChatResponse {
    pub id: Option<String>,
    pub model: Option<String>,
    pub created: Option<u64>,
    pub choices: Vec<OpenAICompatibleChatChoice>,
    pub usage: Option<OpenAICompatibleChatUsage>,
    pub system_fingerprint: Option<String>,
    pub service_tier: Option<String>,
}

/// A single choice in a chat completion response.
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAICompatibleChatChoice {
    pub index: Option<u32>,
    pub message: OpenAICompatibleChatMessage,
    pub finish_reason: Option<String>,
    pub logprobs: Option<OpenAICompatibleChatLogprobs>,
}

/// The message content of a chat completion choice.
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAICompatibleChatMessage {
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<OpenAICompatibleChatToolCall>>,
    pub refusal: Option<String>,
    pub annotations: Option<Vec<OpenAICompatibleChatAnnotation>>,
    /// Reasoning content (used by some providers like DeepSeek).
    pub reasoning_content: Option<String>,
    /// Alternative reasoning field (used by some other providers).
    pub reasoning: Option<String>,
}

/// A tool call in a chat completion response.
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAICompatibleChatToolCall {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    pub function: OpenAICompatibleChatFunctionCall,
    /// Extra content from the provider (e.g., Google's thought_signature).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_content: Option<serde_json::Value>,
}

/// Function call details.
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAICompatibleChatFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Annotation (citation) in a chat message.
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAICompatibleChatAnnotation {
    #[serde(rename = "type")]
    pub annotation_type: Option<String>,
    pub url_citation: Option<OpenAICompatibleUrlCitation>,
}

/// URL citation detail.
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAICompatibleUrlCitation {
    pub url: Option<String>,
    pub title: Option<String>,
    pub start_index: Option<u64>,
    pub end_index: Option<u64>,
}

/// Logprobs data.
#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAICompatibleChatLogprobs {
    pub content: Option<Vec<OpenAICompatibleChatLogprobItem>>,
}

/// A single logprob token entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAICompatibleChatLogprobItem {
    pub token: Option<String>,
    pub logprob: Option<f64>,
    pub bytes: Option<Vec<u8>>,
    pub top_logprobs: Option<Vec<OpenAICompatibleChatTopLogprob>>,
}

/// A top logprob entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAICompatibleChatTopLogprob {
    pub token: Option<String>,
    pub logprob: Option<f64>,
    pub bytes: Option<Vec<u8>>,
}

// --- Streaming types ---

/// A streaming chunk from the Chat Completions API.
#[derive(Debug, Deserialize)]
pub struct OpenAICompatibleChatChunk {
    pub id: Option<String>,
    pub model: Option<String>,
    pub created: Option<u64>,
    pub choices: Option<Vec<OpenAICompatibleChatChunkChoice>>,
    pub usage: Option<OpenAICompatibleChatUsage>,
    pub system_fingerprint: Option<String>,
    pub service_tier: Option<String>,
}

/// A choice within a streaming chunk.
#[derive(Debug, Deserialize)]
pub struct OpenAICompatibleChatChunkChoice {
    pub index: Option<u32>,
    pub delta: OpenAICompatibleChatChunkDelta,
    pub finish_reason: Option<String>,
    pub logprobs: Option<OpenAICompatibleChatLogprobs>,
}

/// Delta content within a streaming chunk choice.
#[derive(Debug, Deserialize)]
pub struct OpenAICompatibleChatChunkDelta {
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<OpenAICompatibleChatChunkToolCall>>,
    pub refusal: Option<String>,
    pub annotations: Option<Vec<OpenAICompatibleChatAnnotation>>,
    /// Reasoning content delta (used by some providers).
    pub reasoning_content: Option<String>,
    /// Alternative reasoning delta field.
    pub reasoning: Option<String>,
}

/// A partial tool call within a streaming chunk.
#[derive(Debug, Deserialize)]
pub struct OpenAICompatibleChatChunkToolCall {
    pub index: Option<u32>,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    pub function: Option<OpenAICompatibleChatChunkFunctionCall>,
    /// Extra content from the provider (e.g., Google's thought_signature).
    pub extra_content: Option<serde_json::Value>,
}

/// Partial function call in a streaming chunk.
#[derive(Debug, Deserialize)]
pub struct OpenAICompatibleChatChunkFunctionCall {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[cfg(test)]
#[path = "openai_compatible_chat_api.test.rs"]
mod tests;
