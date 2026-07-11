use serde::Deserialize;
use serde::Serialize;

use crate::convert_xai_chat_usage::XaiChatUsage;

// A deliberately limited subset of the wire schema — only the fields the
// implementation reads — so upstream API additions don't break parsing.
// Mirrors the zod schemas in `xai-chat-language-model.ts`.

/// Non-streaming Chat Completions response.
#[derive(Debug, Deserialize, Serialize)]
pub struct XaiChatResponse {
    pub id: Option<String>,
    pub created: Option<u64>,
    pub model: Option<String>,
    pub choices: Option<Vec<XaiChatChoice>>,
    pub usage: Option<XaiChatUsage>,
    /// Source URLs surfaced by Live Search / agentic tools.
    pub citations: Option<Vec<String>>,
    /// Set on a soft error delivered with HTTP 200.
    pub code: Option<String>,
    /// Set on a soft error delivered with HTTP 200.
    pub error: Option<String>,
}

/// A single choice in a non-streaming response.
#[derive(Debug, Deserialize, Serialize)]
pub struct XaiChatChoice {
    pub message: XaiResponseMessage,
    pub index: Option<u64>,
    pub finish_reason: Option<String>,
}

/// The assistant message in a non-streaming response.
#[derive(Debug, Deserialize, Serialize)]
pub struct XaiResponseMessage {
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    pub tool_calls: Option<Vec<XaiResponseToolCall>>,
}

/// A tool call in a non-streaming response.
#[derive(Debug, Deserialize, Serialize)]
pub struct XaiResponseToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    pub function: XaiResponseFunction,
}

/// Function call details in a non-streaming response.
#[derive(Debug, Deserialize, Serialize)]
pub struct XaiResponseFunction {
    pub name: String,
    pub arguments: String,
}

// --- Streaming types ---

/// A streaming chunk from the Chat Completions API.
///
/// Unlike Groq, xAI reports usage at the top level (requested via
/// `stream_options.include_usage`).
#[derive(Debug, Deserialize)]
pub struct XaiChatChunk {
    pub id: Option<String>,
    pub created: Option<u64>,
    pub model: Option<String>,
    pub choices: Option<Vec<XaiChatChunkChoice>>,
    pub usage: Option<XaiChatUsage>,
    pub citations: Option<Vec<String>>,
}

/// A choice within a streaming chunk.
#[derive(Debug, Deserialize)]
pub struct XaiChatChunkChoice {
    pub delta: Option<XaiChatChunkDelta>,
    pub finish_reason: Option<String>,
    pub index: Option<u64>,
}

/// Delta content within a streaming chunk choice.
#[derive(Debug, Deserialize)]
pub struct XaiChatChunkDelta {
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    pub tool_calls: Option<Vec<XaiChatChunkToolCall>>,
}

/// A tool call within a streaming chunk. xAI delivers each tool call in a
/// single delta (id + name + full arguments), so there is no cross-delta
/// accumulation.
#[derive(Debug, Deserialize)]
pub struct XaiChatChunkToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    pub function: XaiChatChunkFunction,
}

/// Function call in a streaming chunk.
#[derive(Debug, Deserialize)]
pub struct XaiChatChunkFunction {
    pub name: String,
    pub arguments: String,
}
