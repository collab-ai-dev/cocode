use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Response types (Deserialize for parsing API responses)
// ---------------------------------------------------------------------------

/// Top-level response from `/v1/messages`.
#[derive(Debug, Deserialize)]
pub struct AnthropicMessagesResponse {
    pub id: Option<String>,
    pub model: Option<String>,
    pub content: Vec<AnthropicResponseContentBlock>,
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
    pub usage: Option<AnthropicUsage>,
    pub container: Option<AnthropicResponseContainerRaw>,
    pub context_management: Option<Value>,
}

/// Usage information from Anthropic API.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnthropicUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub iterations: Option<Vec<AnthropicUsageIterationRaw>>,
}

/// A single iteration in the usage breakdown.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnthropicUsageIterationRaw {
    #[serde(rename = "type")]
    pub iteration_type: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Container metadata from the response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnthropicResponseContainerRaw {
    pub id: String,
    pub expires_at: String,
    pub skills: Option<Vec<AnthropicContainerSkillRaw>>,
}

/// A skill loaded in a container.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnthropicContainerSkillRaw {
    #[serde(rename = "type")]
    pub skill_type: String,
    pub skill_id: String,
    pub version: String,
}

/// Citation types.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicCitation {
    #[serde(rename = "web_search_result_location")]
    WebSearchResultLocation {
        cited_text: String,
        url: String,
        title: String,
        encrypted_index: String,
    },
    #[serde(rename = "page_location")]
    PageLocation {
        cited_text: String,
        document_index: u64,
        document_title: Option<String>,
        start_page_number: u64,
        end_page_number: u64,
    },
    #[serde(rename = "char_location")]
    CharLocation {
        cited_text: String,
        document_index: u64,
        document_title: Option<String>,
        start_char_index: u64,
        end_char_index: u64,
    },
    /// Fallback for unknown citation types from the API.
    #[serde(other)]
    Unknown,
}

/// Content block in a response.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicResponseContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        citations: Option<Vec<AnthropicCitation>>,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(rename = "compaction")]
    Compaction { content: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
        caller: Option<Value>,
    },
    #[serde(rename = "server_tool_use")]
    ServerToolUse {
        id: String,
        name: String,
        input: Option<Value>,
    },
    #[serde(rename = "mcp_tool_use")]
    McpToolUse {
        id: String,
        name: String,
        server_name: String,
        input: Value,
    },
    #[serde(rename = "mcp_tool_result")]
    McpToolResult {
        tool_use_id: String,
        is_error: bool,
        content: Value,
    },
    #[serde(rename = "web_search_tool_result")]
    WebSearchToolResult { tool_use_id: String, content: Value },
    #[serde(rename = "web_fetch_tool_result")]
    WebFetchToolResult { tool_use_id: String, content: Value },
    #[serde(rename = "code_execution_tool_result")]
    CodeExecutionToolResult { tool_use_id: String, content: Value },
    #[serde(rename = "bash_code_execution_tool_result")]
    BashCodeExecutionToolResult { tool_use_id: String, content: Value },
    #[serde(rename = "text_editor_code_execution_tool_result")]
    TextEditorCodeExecutionToolResult { tool_use_id: String, content: Value },
    #[serde(rename = "tool_search_tool_result")]
    ToolSearchToolResult { tool_use_id: String, content: Value },
    /// Fallback for unknown content block types from the API.
    #[serde(other)]
    Unknown,
}

// ---------------------------------------------------------------------------
// Streaming types (SSE events)
// ---------------------------------------------------------------------------

/// `message_start` event data.
#[derive(Debug, Deserialize)]
pub struct MessageStartEvent {
    pub message: MessageStartMessage,
}

#[derive(Debug, Deserialize)]
pub struct MessageStartMessage {
    pub id: Option<String>,
    pub model: Option<String>,
    pub usage: Option<MessageStartUsage>,
    pub container: Option<AnthropicResponseContainerRaw>,
    pub stop_reason: Option<String>,
    pub content: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MessageStartUsage {
    pub input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
}

/// `content_block_start` event data.
#[derive(Debug, Deserialize)]
pub struct ContentBlockStartEvent {
    pub index: u64,
    pub content_block: ContentBlockStart,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlockStart {
    #[serde(rename = "text")]
    Text { text: Option<String> },
    #[serde(rename = "thinking")]
    Thinking { thinking: Option<String> },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: Option<String> },
    #[serde(rename = "compaction")]
    Compaction { content: Option<String> },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Option<Value>,
        caller: Option<Value>,
    },
    #[serde(rename = "server_tool_use")]
    ServerToolUse {
        id: String,
        name: String,
        input: Option<Value>,
    },
    #[serde(rename = "mcp_tool_use")]
    McpToolUse {
        id: String,
        name: String,
        server_name: String,
        input: Option<Value>,
    },
    #[serde(rename = "web_search_tool_result")]
    WebSearchToolResult { id: Option<String> },
    #[serde(rename = "web_fetch_tool_result")]
    WebFetchToolResult { id: Option<String> },
    #[serde(rename = "code_execution_tool_result")]
    CodeExecutionToolResult { id: Option<String> },
    #[serde(rename = "bash_code_execution_tool_result")]
    BashCodeExecutionToolResult { id: Option<String> },
    #[serde(rename = "text_editor_code_execution_tool_result")]
    TextEditorCodeExecutionToolResult { id: Option<String> },
    #[serde(rename = "mcp_tool_result")]
    McpToolResult {
        tool_use_id: Option<String>,
        is_error: Option<bool>,
    },
    #[serde(rename = "tool_search_tool_result")]
    ToolSearchToolResult { id: Option<String> },
    /// Fallback for unknown content block start types.
    #[serde(other)]
    Unknown,
}

/// `content_block_delta` event data.
#[derive(Debug, Deserialize)]
pub struct ContentBlockDeltaEvent {
    pub index: u64,
    pub delta: ContentBlockDelta,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlockDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "citations_delta")]
    CitationsDelta { citation: AnthropicCitation },
    // Server tool result deltas — pass through as raw JSON
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta {
        #[serde(flatten)]
        data: Value,
    },
    /// Fallback for unknown delta types.
    #[serde(other)]
    Unknown,
}

/// `content_block_stop` event data.
#[derive(Debug, Deserialize)]
pub struct ContentBlockStopEvent {
    pub index: u64,
    pub content_block: Option<Value>,
}

/// `message_delta` event data.
#[derive(Debug, Deserialize)]
pub struct MessageDeltaEvent {
    pub delta: MessageDelta,
    pub usage: Option<MessageDeltaUsage>,
    pub context_management: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct MessageDelta {
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
    pub container: Option<AnthropicResponseContainerRaw>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MessageDeltaUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub iterations: Option<Vec<AnthropicUsageIterationRaw>>,
}

/// SSE error event.
#[derive(Debug, Deserialize)]
pub struct StreamErrorEvent {
    #[serde(rename = "type")]
    pub error_type: Option<String>,
    pub error: Option<StreamErrorDetail>,
}

#[derive(Debug, Deserialize)]
pub struct StreamErrorDetail {
    #[serde(rename = "type")]
    pub error_type: Option<String>,
    pub message: Option<String>,
}

#[cfg(test)]
#[path = "anthropic_messages_api.test.rs"]
mod tests;
