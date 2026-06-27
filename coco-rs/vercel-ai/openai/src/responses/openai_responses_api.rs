use serde::Deserialize;
use serde::de;
use serde_json::Value;

use super::convert_responses_usage::OpenAIResponsesUsage;

/// Deserialize `created_at` which may be a Unix timestamp (number) or an ISO 8601 string.
fn deserialize_created_at<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: de::Deserializer<'de>,
{
    let val = Option::<Value>::deserialize(deserializer)?;
    Ok(match val {
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|ts| chrono::DateTime::from_timestamp(ts as i64, 0))
            .map(|dt| dt.to_rfc3339()),
        Some(Value::String(s)) => Some(s),
        _ => None,
    })
}

/// Non-streaming response from the Responses API.
#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesResponse {
    pub id: Option<String>,
    pub model: Option<String>,
    #[serde(default, deserialize_with = "deserialize_created_at")]
    pub created_at: Option<String>,
    pub output: Vec<ResponseOutputItem>,
    pub usage: Option<OpenAIResponsesUsage>,
    pub status: Option<String>,
    pub service_tier: Option<String>,
    pub incomplete_details: Option<Value>,
}

/// An output item from the Responses API.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ResponseOutputItem {
    #[serde(rename = "message")]
    Message {
        id: Option<String>,
        role: Option<String>,
        content: Vec<ResponseMessageContent>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        id: Option<String>,
        /// Wire `call_id` (`call_…`) — the correlation id echoed back as the
        /// `function_call_output.call_id`. Mandatory per the Responses API
        /// contract (mirrors codex `ResponseItem::FunctionCall.call_id:
        /// String`); a `function_call` item without it fails to decode and the
        /// SSE event is skipped (graceful per-event drop in `process_event`).
        call_id: String,
        name: Option<String>,
        arguments: Option<String>,
        /// Namespace marker — present when the function_call originated
        /// from a server-executed tool_search dispatching to a deferred
        /// client-side tool. Surfaces as
        /// `provider_metadata.openai.namespace` on the resulting
        /// `tool-call` part. (TS upstream #14789)
        #[serde(default)]
        namespace: Option<String>,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput {
        id: Option<String>,
        call_id: Option<String>,
        output: Option<String>,
    },
    #[serde(rename = "custom_tool_call")]
    CustomToolCall {
        id: Option<String>,
        /// Wire `call_id` — echoed back as `custom_tool_call_output.call_id`.
        /// Mandatory per the Responses API contract (mirrors codex's mandatory
        /// `call_id: String`); a malformed item without it is skipped at the
        /// SSE-event boundary rather than producing an unaddressable call.
        call_id: String,
        name: Option<String>,
        input: Option<String>,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        id: Option<String>,
        summary: Option<Vec<ReasoningSummaryItem>>,
        /// Raw reasoning text (the `content` channel), distinct from the
        /// condensed `summary`. Present only on models/configs that stream
        /// `response.reasoning_text.*`. Display-only — never re-sent.
        #[serde(default)]
        content: Option<Vec<ReasoningContentItem>>,
        encrypted_content: Option<Value>,
    },
    #[serde(rename = "computer_call")]
    ComputerCall {
        id: Option<String>,
        call_id: Option<String>,
        #[serde(flatten)]
        rest: Value,
    },
    #[serde(rename = "web_search_call")]
    WebSearchCall {
        id: Option<String>,
        status: Option<String>,
    },
    #[serde(rename = "file_search_call")]
    FileSearchCall {
        id: Option<String>,
        status: Option<String>,
        results: Option<Vec<Value>>,
    },
    #[serde(rename = "code_interpreter_call")]
    CodeInterpreterCall {
        id: Option<String>,
        status: Option<String>,
        code: Option<String>,
        outputs: Option<Vec<Value>>,
    },
    #[serde(rename = "image_generation_call")]
    ImageGenerationCall {
        id: Option<String>,
        result: Option<Value>,
    },
    #[serde(rename = "mcp_call")]
    McpCall {
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
        server_label: Option<String>,
        output: Option<Value>,
        error: Option<Value>,
    },
    #[serde(rename = "mcp_approval_request")]
    McpApprovalRequest {
        id: Option<String>,
        #[serde(flatten)]
        rest: Value,
    },
    #[serde(rename = "mcp_list_tools")]
    McpListTools {
        id: Option<String>,
        #[serde(flatten)]
        rest: Value,
    },
    #[serde(rename = "local_shell_call")]
    LocalShellCall {
        id: Option<String>,
        call_id: Option<String>,
        action: Option<Value>,
        status: Option<String>,
    },
    #[serde(rename = "shell_call")]
    ShellCall {
        id: Option<String>,
        call_id: Option<String>,
        action: Option<Value>,
        status: Option<String>,
        output: Option<Vec<Value>>,
    },
    #[serde(rename = "shell_call_output")]
    ShellCallOutput {
        id: Option<String>,
        call_id: Option<String>,
        output: Option<Vec<Value>>,
    },
    #[serde(rename = "apply_patch_call")]
    ApplyPatchCall {
        id: Option<String>,
        call_id: Option<String>,
        operation: Option<Value>,
        status: Option<String>,
    },
    #[serde(rename = "tool_search_call")]
    ToolSearchCall {
        id: Option<String>,
        execution: Option<String>,
        call_id: Option<String>,
        status: Option<String>,
        arguments: Option<Value>,
    },
    #[serde(rename = "tool_search_output")]
    ToolSearchOutput {
        id: Option<String>,
        execution: Option<String>,
        call_id: Option<String>,
        status: Option<String>,
        tools: Option<Vec<Value>>,
    },
    #[serde(rename = "compaction")]
    Compaction {
        id: Option<String>,
        encrypted_content: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

/// Content within a message output item.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ResponseMessageContent {
    #[serde(rename = "output_text")]
    OutputText {
        text: Option<String>,
        annotations: Option<Vec<ResponseAnnotation>>,
        logprobs: Option<Vec<Value>>,
    },
    #[serde(rename = "refusal")]
    Refusal { refusal: Option<String> },
    #[serde(other)]
    Unknown,
}

/// An annotation on text output.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ResponseAnnotation {
    #[serde(rename = "url_citation")]
    UrlCitation {
        url: Option<String>,
        title: Option<String>,
        start_index: Option<u64>,
        end_index: Option<u64>,
    },
    #[serde(rename = "file_citation")]
    FileCitation {
        file_id: Option<String>,
        index: Option<u64>,
    },
    #[serde(rename = "file_path")]
    FilePath {
        file_id: Option<String>,
        index: Option<u64>,
    },
    #[serde(rename = "container_file_citation")]
    ContainerFileCitation {
        file_id: Option<String>,
        container_id: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReasoningSummaryItem {
    #[serde(rename = "type")]
    pub item_type: Option<String>,
    pub text: Option<String>,
}

/// One raw reasoning `content` entry (the `reasoning_text` channel).
#[derive(Debug, Clone, Deserialize)]
pub struct ReasoningContentItem {
    #[serde(rename = "type")]
    pub item_type: Option<String>,
    pub text: Option<String>,
}

// --- Streaming event types ---

/// A streaming event from the Responses API.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ResponsesStreamEvent {
    #[serde(rename = "response.created")]
    ResponseCreated { response: Option<ResponseMeta> },

    #[serde(rename = "response.in_progress")]
    ResponseInProgress { response: Option<ResponseMeta> },

    #[serde(rename = "response.completed")]
    ResponseCompleted { response: Option<ResponseMeta> },

    #[serde(rename = "response.failed")]
    ResponseFailed { response: Option<ResponseMeta> },

    #[serde(rename = "response.incomplete")]
    ResponseIncomplete { response: Option<ResponseMeta> },

    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        item_id: Option<String>,
        delta: Option<String>,
        logprobs: Option<Vec<Value>>,
    },

    #[serde(rename = "response.output_text.done")]
    OutputTextDone {
        item_id: Option<String>,
        text: Option<String>,
    },

    #[serde(rename = "response.output_text.annotation.added")]
    OutputTextAnnotationAdded {
        item_id: Option<String>,
        annotation: Option<ResponseAnnotation>,
    },

    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {
        item_id: Option<String>,
        part: Option<Value>,
    },

    #[serde(rename = "response.content_part.done")]
    ContentPartDone {
        item_id: Option<String>,
        part: Option<Value>,
    },

    #[serde(rename = "response.output_item.added")]
    OutputItemAdded { item: Option<ResponseOutputItem> },

    #[serde(rename = "response.output_item.done")]
    OutputItemDone { item: Option<ResponseOutputItem> },

    #[serde(rename = "response.function_call_arguments.delta")]
    FnCallArgsDelta {
        item_id: Option<String>,
        delta: Option<String>,
    },

    #[serde(rename = "response.function_call_arguments.done")]
    FnCallArgsDone {
        item_id: Option<String>,
        arguments: Option<String>,
    },

    #[serde(rename = "response.custom_tool_call_input.delta")]
    CustomToolCallInputDelta {
        item_id: Option<String>,
        // The wire also carries `call_id` here, but it's redundant: the active
        // call is created (with its mandatory `call_id`) by `output_item.added`
        // before any delta, so the delta only needs `item_id` to find it. The
        // extra wire field is ignored.
        delta: Option<String>,
    },

    #[serde(rename = "response.custom_tool_call_input.done")]
    CustomToolCallInputDone {
        item_id: Option<String>,
        input: Option<String>,
    },

    #[serde(rename = "response.reasoning_summary_part.added")]
    ReasoningSummaryPartAdded {
        item_id: Option<String>,
        #[serde(default)]
        summary_index: Option<u64>,
        part: Option<Value>,
    },

    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningTextDelta {
        item_id: Option<String>,
        /// Wire ordinal; deserialized for completeness, not consumed here.
        #[serde(default)]
        content_index: Option<u64>,
        delta: Option<String>,
    },

    #[serde(rename = "response.reasoning_text.done")]
    ReasoningTextDone {
        item_id: Option<String>,
        /// Wire ordinal; deserialized for completeness, not consumed here.
        #[serde(default)]
        content_index: Option<u64>,
        text: Option<String>,
    },

    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryDelta {
        item_id: Option<String>,
        delta: Option<String>,
    },

    #[serde(rename = "response.reasoning_summary_text.done")]
    ReasoningSummaryDone {
        item_id: Option<String>,
        text: Option<String>,
    },

    #[serde(rename = "response.reasoning_summary_part.done")]
    ReasoningSummaryPartDone {
        item_id: Option<String>,
        part: Option<Value>,
    },

    #[serde(rename = "response.code_interpreter_call_code.delta")]
    CodeInterpreterCodeDelta {
        item_id: Option<String>,
        delta: Option<String>,
    },

    #[serde(rename = "response.code_interpreter_call_code.done")]
    CodeInterpreterCodeDone {
        item_id: Option<String>,
        code: Option<String>,
    },

    #[serde(rename = "response.image_generation_call.partial_image")]
    ImageGenerationPartialImage {
        item_id: Option<String>,
        partial_image_index: Option<u32>,
        partial_image_b64: Option<String>,
    },

    #[serde(rename = "response.apply_patch_call_operation_diff.delta")]
    ApplyPatchDiffDelta {
        item_id: Option<String>,
        delta: Option<String>,
    },

    #[serde(rename = "response.apply_patch_call_operation_diff.done")]
    ApplyPatchDiffDone {
        item_id: Option<String>,
        diff: Option<String>,
    },

    #[serde(rename = "error")]
    Error {
        message: Option<String>,
        code: Option<String>,
    },

    #[serde(other)]
    Unknown,
}

/// Response metadata included in streaming events.
#[derive(Debug, Clone, Deserialize)]
pub struct ResponseMeta {
    pub id: Option<String>,
    pub model: Option<String>,
    #[serde(default, deserialize_with = "deserialize_created_at")]
    pub created_at: Option<String>,
    pub usage: Option<OpenAIResponsesUsage>,
    pub status: Option<String>,
    pub service_tier: Option<String>,
    pub output: Option<Vec<ResponseOutputItem>>,
    pub incomplete_details: Option<Value>,
    pub error: Option<Value>,
}

#[cfg(test)]
#[path = "openai_responses_api.test.rs"]
mod tests;
