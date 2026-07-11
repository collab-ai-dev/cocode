use serde::Deserialize;
use serde::Serialize;

use super::convert_xai_responses_usage::XaiResponsesUsage;

// A deliberately limited subset of the wire schema — only the fields the
// implementation reads — so upstream API additions don't break parsing.
// Mirrors the zod schemas in `xai-responses-api.ts`.

/// Non-streaming response from the Responses API.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct XaiResponsesResponse {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub created_at: Option<i64>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub output: Vec<ResponseOutputItem>,
    #[serde(default)]
    pub usage: Option<XaiResponsesUsage>,
    #[serde(default)]
    pub status: Option<String>,
    /// Set on a soft error delivered with HTTP 200 (`{code, error}` shape).
    #[serde(default)]
    pub code: Option<String>,
    /// Set on a soft error delivered with HTTP 200 (`{code, error}` shape).
    #[serde(default)]
    pub error: Option<String>,
}

/// An output item from the Responses API.
///
/// The server-side agentic tool calls (`web_search_call`, `x_search_call`, …)
/// all share the same wire shape, so they wrap a common
/// [`ServerToolCallItem`]; `mcp_call` carries the same relevant fields.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum ResponseOutputItem {
    #[serde(rename = "message")]
    Message {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        role: Option<String>,
        #[serde(default)]
        content: Vec<ResponseMessageContentPart>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        #[serde(default)]
        id: Option<String>,
        /// Wire `call_id` (`call_…`) — the correlation id echoed back as the
        /// `function_call_output.call_id`. Mandatory per the Responses API
        /// contract; an item without it fails to decode and the SSE event is
        /// skipped (graceful per-event drop).
        call_id: String,
        name: String,
        #[serde(default)]
        arguments: String,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        summary: Vec<ReasoningSummaryPart>,
        /// Raw reasoning text (the `content` channel), distinct from the
        /// condensed `summary`. Present only on configs that stream
        /// `response.reasoning_text.*`.
        #[serde(default)]
        content: Option<Vec<ReasoningTextPart>>,
        #[serde(default)]
        encrypted_content: Option<String>,
    },
    #[serde(rename = "file_search_call")]
    FileSearchCall {
        id: String,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        queries: Option<Vec<String>>,
        #[serde(default)]
        results: Option<Vec<FileSearchResult>>,
    },
    #[serde(rename = "web_search_call")]
    WebSearchCall(ServerToolCallItem),
    #[serde(rename = "x_search_call")]
    XSearchCall(ServerToolCallItem),
    #[serde(rename = "code_interpreter_call")]
    CodeInterpreterCall(ServerToolCallItem),
    #[serde(rename = "code_execution_call")]
    CodeExecutionCall(ServerToolCallItem),
    #[serde(rename = "view_image_call")]
    ViewImageCall(ServerToolCallItem),
    #[serde(rename = "view_x_video_call")]
    ViewXVideoCall(ServerToolCallItem),
    #[serde(rename = "custom_tool_call")]
    CustomToolCall(ServerToolCallItem),
    #[serde(rename = "mcp_call")]
    McpCall(ServerToolCallItem),
    #[serde(other)]
    Unknown,
}

/// Common wire shape for the server-side agentic tool-call output items.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerToolCallItem {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// Content within a message output item. Only `text` and `annotations` are
/// read; the discriminating `type` (output_text / refusal) is ignored.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResponseMessageContentPart {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub annotations: Option<Vec<ResponseAnnotation>>,
}

/// An annotation on text output.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum ResponseAnnotation {
    #[serde(rename = "url_citation")]
    UrlCitation {
        url: String,
        #[serde(default)]
        title: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

/// One condensed reasoning summary entry (`summary_text`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReasoningSummaryPart {
    #[serde(default)]
    pub text: String,
}

/// One raw reasoning `content` entry (the `reasoning_text` channel).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReasoningTextPart {
    #[serde(default)]
    pub text: String,
}

/// A single result from a `file_search_call` output item.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileSearchResult {
    #[serde(default)]
    pub file_id: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub text: Option<String>,
}

impl ResponseAnnotation {
    /// Return the URL of a `url_citation` annotation, if this is one.
    pub fn url_citation(&self) -> Option<(&str, Option<&str>)> {
        match self {
            ResponseAnnotation::UrlCitation { url, title } => Some((url, title.as_deref())),
            ResponseAnnotation::Unknown => None,
        }
    }
}

/// The kind of server-side (provider-executed) agentic tool an output item
/// represents. Drives the tool-name resolution shared by `do_generate` and the
/// streaming state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerToolKind {
    WebSearch,
    XSearch,
    CodeInterpreter,
    CodeExecution,
    ViewImage,
    ViewXVideo,
    CustomToolCall,
    Mcp,
}

/// The resolved user-facing names for the provider-defined tools that were
/// declared on the request, keyed by their default id. `do_generate` /
/// `do_stream` map a server-executed output item back onto the caller's tool
/// name via these overrides (falling back to the canonical default name).
#[derive(Debug, Clone, Default)]
pub struct ResponsesToolNames {
    pub web_search: Option<String>,
    pub x_search: Option<String>,
    pub code_execution: Option<String>,
    pub mcp: Option<String>,
    pub file_search: Option<String>,
}

impl ResponseOutputItem {
    /// If this item is a server-side tool call, return its kind and payload.
    pub fn as_server_tool(&self) -> Option<(ServerToolKind, &ServerToolCallItem)> {
        match self {
            ResponseOutputItem::WebSearchCall(i) => Some((ServerToolKind::WebSearch, i)),
            ResponseOutputItem::XSearchCall(i) => Some((ServerToolKind::XSearch, i)),
            ResponseOutputItem::CodeInterpreterCall(i) => {
                Some((ServerToolKind::CodeInterpreter, i))
            }
            ResponseOutputItem::CodeExecutionCall(i) => Some((ServerToolKind::CodeExecution, i)),
            ResponseOutputItem::ViewImageCall(i) => Some((ServerToolKind::ViewImage, i)),
            ResponseOutputItem::ViewXVideoCall(i) => Some((ServerToolKind::ViewXVideo, i)),
            ResponseOutputItem::CustomToolCall(i) => Some((ServerToolKind::CustomToolCall, i)),
            ResponseOutputItem::McpCall(i) => Some((ServerToolKind::Mcp, i)),
            _ => None,
        }
    }
}

const WEB_SEARCH_SUB_TOOLS: [&str; 3] = ["web_search", "web_search_with_snippets", "browse_page"];
const X_SEARCH_SUB_TOOLS: [&str; 4] = [
    "x_user_search",
    "x_keyword_search",
    "x_semantic_search",
    "x_thread_fetch",
];

/// Resolve a server-tool output item to `(tool_name, input)`, mirroring the
/// name-override cascade in `xai-responses-language-model.ts`.
pub fn resolve_server_tool(
    kind: ServerToolKind,
    item: &ServerToolCallItem,
    names: &ResponsesToolNames,
) -> (String, String) {
    let name = item.name.as_deref();
    let is_web = matches!(kind, ServerToolKind::WebSearch)
        || name.is_some_and(|n| WEB_SEARCH_SUB_TOOLS.contains(&n));
    let is_x = matches!(kind, ServerToolKind::XSearch)
        || name.is_some_and(|n| X_SEARCH_SUB_TOOLS.contains(&n));
    let is_code = name == Some("code_execution")
        || matches!(
            kind,
            ServerToolKind::CodeInterpreter | ServerToolKind::CodeExecution
        );

    let tool_name = if is_web {
        names
            .web_search
            .clone()
            .unwrap_or_else(|| "web_search".to_string())
    } else if is_x {
        names
            .x_search
            .clone()
            .unwrap_or_else(|| "x_search".to_string())
    } else if is_code {
        names
            .code_execution
            .clone()
            .unwrap_or_else(|| "code_execution".to_string())
    } else if matches!(kind, ServerToolKind::Mcp) {
        names
            .mcp
            .clone()
            .or_else(|| name.map(String::from))
            .unwrap_or_else(|| "mcp".to_string())
    } else {
        name.unwrap_or("").to_string()
    };

    let input = match kind {
        ServerToolKind::CustomToolCall => item.input.clone().unwrap_or_default(),
        _ => item.arguments.clone().unwrap_or_default(),
    };

    (tool_name, input)
}

// --- Streaming event types ---

/// A streaming event from the Responses API. Sub-progress events
/// (`response.*.in_progress`, `.searching`, `.completed`, …) that the state
/// machine does not act on fall through to [`ResponsesStreamEvent::Unknown`].
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ResponsesStreamEvent {
    #[serde(rename = "response.created")]
    ResponseCreated { response: Option<ResponseMeta> },

    #[serde(rename = "response.in_progress")]
    ResponseInProgress { response: Option<ResponseMeta> },

    #[serde(rename = "response.completed")]
    ResponseCompleted { response: Option<ResponseMeta> },

    #[serde(rename = "response.done")]
    ResponseDone { response: Option<ResponseMeta> },

    #[serde(rename = "response.incomplete")]
    ResponseIncomplete { response: Option<ResponseMeta> },

    #[serde(rename = "response.failed")]
    ResponseFailed { response: Option<ResponseMeta> },

    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        item: Option<ResponseOutputItem>,
        #[serde(default)]
        output_index: u64,
    },

    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        item: Option<ResponseOutputItem>,
        #[serde(default)]
        output_index: u64,
    },

    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        item_id: String,
        #[serde(default)]
        delta: String,
    },

    #[serde(rename = "response.output_text.done")]
    OutputTextDone {
        #[serde(default)]
        item_id: Option<String>,
        #[serde(default)]
        annotations: Option<Vec<ResponseAnnotation>>,
    },

    #[serde(rename = "response.output_text.annotation.added")]
    OutputTextAnnotationAdded {
        #[serde(default)]
        item_id: Option<String>,
        annotation: ResponseAnnotation,
    },

    #[serde(rename = "response.reasoning_summary_part.added")]
    ReasoningSummaryPartAdded { item_id: String },

    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta {
        item_id: String,
        #[serde(default)]
        delta: String,
    },

    #[serde(rename = "response.reasoning_summary_text.done")]
    ReasoningSummaryTextDone {
        #[serde(default)]
        item_id: Option<String>,
    },

    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningTextDelta {
        item_id: String,
        #[serde(default)]
        delta: String,
    },

    #[serde(rename = "response.reasoning_text.done")]
    ReasoningTextDone {
        #[serde(default)]
        item_id: Option<String>,
    },

    #[serde(rename = "response.function_call_arguments.delta")]
    FnCallArgsDelta {
        #[serde(default)]
        item_id: Option<String>,
        #[serde(default)]
        output_index: u64,
        #[serde(default)]
        delta: String,
    },

    #[serde(rename = "response.function_call_arguments.done")]
    FnCallArgsDone {
        #[serde(default)]
        item_id: Option<String>,
        #[serde(default)]
        output_index: u64,
    },

    #[serde(rename = "response.custom_tool_call_input.delta")]
    CustomToolCallInputDelta {
        #[serde(default)]
        item_id: Option<String>,
    },

    #[serde(rename = "response.custom_tool_call_input.done")]
    CustomToolCallInputDone {
        #[serde(default)]
        item_id: Option<String>,
    },

    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        code: Option<String>,
    },

    #[serde(other)]
    Unknown,
}

/// Response metadata carried by streaming lifecycle events.
#[derive(Debug, Clone, Deserialize)]
pub struct ResponseMeta {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub created_at: Option<i64>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub usage: Option<XaiResponsesUsage>,
    #[serde(default)]
    pub incomplete_details: Option<IncompleteDetails>,
    #[serde(default)]
    pub error: Option<ResponseErrorDetail>,
}

/// The `incomplete_details` object carrying the real stop reason.
#[derive(Debug, Clone, Deserialize)]
pub struct IncompleteDetails {
    #[serde(default)]
    pub reason: Option<String>,
}

/// The `error` object on a `response.failed` event.
#[derive(Debug, Clone, Deserialize)]
pub struct ResponseErrorDetail {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[cfg(test)]
#[path = "xai_responses_api_types.test.rs"]
mod tests;
