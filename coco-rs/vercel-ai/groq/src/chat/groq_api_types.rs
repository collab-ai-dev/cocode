use serde::Deserialize;
use serde::Serialize;

use crate::convert_groq_usage::GroqUsage;

// A deliberately limited subset of the wire schema — only the fields the
// implementation reads — so upstream API additions don't break parsing.
// Mirrors the zod schemas in `groq-chat-language-model.ts`.

/// Non-streaming Chat Completions response.
#[derive(Debug, Deserialize, Serialize)]
pub struct GroqChatResponse {
    pub id: Option<String>,
    pub created: Option<u64>,
    pub model: Option<String>,
    pub choices: Vec<GroqChatChoice>,
    pub usage: Option<GroqUsage>,
}

/// A single choice in a non-streaming response.
#[derive(Debug, Deserialize, Serialize)]
pub struct GroqChatChoice {
    pub message: GroqResponseMessage,
    pub index: Option<u64>,
    pub finish_reason: Option<String>,
}

/// The assistant message in a non-streaming response.
#[derive(Debug, Deserialize, Serialize)]
pub struct GroqResponseMessage {
    pub content: Option<String>,
    pub reasoning: Option<String>,
    pub tool_calls: Option<Vec<GroqResponseToolCall>>,
}

/// A tool call in a non-streaming response.
#[derive(Debug, Deserialize, Serialize)]
pub struct GroqResponseToolCall {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    pub function: GroqResponseFunction,
}

/// Function call details in a non-streaming response.
#[derive(Debug, Deserialize, Serialize)]
pub struct GroqResponseFunction {
    pub name: String,
    pub arguments: String,
}

// --- Streaming types ---

/// A streaming chunk from the Chat Completions API.
///
/// Groq reports usage under `x_groq.usage` (not the top-level `usage`).
#[derive(Debug, Deserialize)]
pub struct GroqChatChunk {
    pub id: Option<String>,
    pub created: Option<u64>,
    pub model: Option<String>,
    pub choices: Option<Vec<GroqChatChunkChoice>>,
    pub x_groq: Option<GroqChatChunkXGroq>,
}

/// The `x_groq` extension object carrying streaming usage.
#[derive(Debug, Deserialize)]
pub struct GroqChatChunkXGroq {
    pub usage: Option<GroqUsage>,
}

/// A choice within a streaming chunk.
#[derive(Debug, Deserialize)]
pub struct GroqChatChunkChoice {
    pub delta: Option<GroqChatChunkDelta>,
    pub finish_reason: Option<String>,
    pub index: Option<u64>,
}

/// Delta content within a streaming chunk choice.
#[derive(Debug, Deserialize)]
pub struct GroqChatChunkDelta {
    pub content: Option<String>,
    pub reasoning: Option<String>,
    pub tool_calls: Option<Vec<GroqChatChunkToolCall>>,
}

/// A partial tool call within a streaming chunk.
#[derive(Debug, Deserialize)]
pub struct GroqChatChunkToolCall {
    pub index: Option<u64>,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    pub function: Option<GroqChatChunkFunction>,
}

/// Partial function call in a streaming chunk.
#[derive(Debug, Deserialize)]
pub struct GroqChatChunkFunction {
    pub name: Option<String>,
    pub arguments: Option<String>,
}
