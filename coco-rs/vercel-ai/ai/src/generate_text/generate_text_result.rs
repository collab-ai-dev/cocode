//! Result types for generate_text.
//!
//! This module defines the result types returned by `generate_text`.

use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FinishReason;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::Source;
use vercel_ai_provider::Usage;
use vercel_ai_provider::Warning;

use crate::types::JSONValue;
use crate::types::LanguageModelRequestMetadata;
use crate::types::LanguageModelResponseMetadata;

use super::content_utils;
use super::generated_file::GeneratedFile;
use super::reasoning_output::ReasoningOutput;
use super::step_result::StepResult;

/// Result of a `generate_text` call.
#[derive(Debug)]
#[must_use]
pub struct GenerateTextResult {
    /// Unique call ID for this generation session.
    pub call_id: String,
    /// The generated text content.
    pub text: String,
    /// The content parts from the response.
    pub content: Vec<AssistantContentPart>,
    /// Structured reasoning outputs (with text, signature, provider metadata).
    pub reasoning: Vec<ReasoningOutput>,
    /// Token usage for this response.
    pub usage: Usage,
    /// Total token usage including previous steps.
    pub total_usage: Usage,
    /// The finish reason.
    pub finish_reason: FinishReason,
    /// Warnings from the provider.
    pub warnings: Vec<Warning>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
    /// Tool calls made during generation.
    pub tool_calls: Vec<ToolCall>,
    /// Tool results from executed tools.
    pub tool_results: Vec<ToolResult>,
    /// Steps taken during generation (for multi-step tool calls).
    pub steps: Vec<StepResult>,
    /// The model ID used.
    pub model_id: Option<String>,
    /// Response timestamp (parsed from the provider — typed as
    /// `DateTime<Utc>` to mirror TS `LanguageModelV4ResponseMetadata.timestamp: Date`).
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// Response headers.
    pub response_headers: Option<std::collections::HashMap<String, String>>,
    /// Sources used in generation (e.g., from RAG).
    pub sources: Vec<Source>,
    /// Generated files (if any).
    pub files: Vec<GeneratedFile>,
    /// Request metadata.
    pub request: Option<LanguageModelRequestMetadata>,
    /// Response metadata.
    pub response: Option<LanguageModelResponseMetadata>,
    /// Structured output (if output parameter was specified).
    pub output: Option<JSONValue>,
}

impl GenerateTextResult {
    /// Create a new generate text result.
    pub fn new(text: String, usage: Usage, finish_reason: FinishReason) -> Self {
        Self {
            call_id: String::new(),
            text,
            content: Vec::new(),
            reasoning: Vec::new(),
            usage: usage.clone(),
            total_usage: usage,
            finish_reason,
            warnings: Vec::new(),
            provider_metadata: None,
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            steps: Vec::new(),
            model_id: None,
            timestamp: None,
            response_headers: None,
            sources: Vec::new(),
            files: Vec::new(),
            request: None,
            response: None,
            output: None,
        }
    }

    /// Create from a LanguageModelV4GenerateResult.
    pub fn from_generate_result(result: LanguageModelV4GenerateResult, model_id: &str) -> Self {
        let text = content_utils::extract_text(&result.content);
        let reasoning = content_utils::extract_reasoning_outputs(&result.content);
        let tool_calls = content_utils::extract_tool_calls(&result.content);

        let response = result.response.as_ref().map(|r| {
            let mut meta = LanguageModelResponseMetadata::new()
                .with_model_id(r.model_id.clone().unwrap_or_default())
                .with_headers(r.headers.clone().unwrap_or_default());
            if let Some(id) = r.id.clone() {
                meta = meta.with_id(id);
            }
            if let Some(ts) = r.timestamp {
                meta = meta.with_timestamp(ts);
            }
            meta
        });

        Self {
            call_id: String::new(),
            text,
            content: result.content,
            reasoning,
            usage: result.usage.clone(),
            total_usage: result.usage,
            finish_reason: result.finish_reason,
            warnings: result.warnings,
            provider_metadata: result.provider_metadata,
            tool_calls,
            tool_results: Vec::new(),
            steps: Vec::new(),
            model_id: Some(model_id.to_string()),
            timestamp: result.response.as_ref().and_then(|r| r.timestamp),
            response_headers: result.response.and_then(|r| r.headers),
            sources: Vec::new(),
            files: Vec::new(),
            request: None,
            response,
            output: None,
        }
    }

    /// Add tool results.
    pub fn with_tool_results(mut self, results: Vec<ToolResult>) -> Self {
        self.tool_results = results;
        self
    }

    /// Add steps.
    pub fn with_steps(mut self, steps: Vec<StepResult>) -> Self {
        self.steps = steps;
        self
    }

    /// Add sources.
    pub fn with_sources(mut self, sources: Vec<Source>) -> Self {
        self.sources = sources;
        self
    }

    /// Add generated files.
    pub fn with_files(mut self, files: Vec<GeneratedFile>) -> Self {
        self.files = files;
        self
    }

    /// Add request metadata.
    pub fn with_request(mut self, request: LanguageModelRequestMetadata) -> Self {
        self.request = Some(request);
        self
    }

    /// Add response metadata.
    pub fn with_response(mut self, response: LanguageModelResponseMetadata) -> Self {
        self.response = Some(response);
        self
    }

    /// Add structured output.
    pub fn with_output(mut self, output: JSONValue) -> Self {
        self.output = Some(output);
        self
    }

    /// Check if there are tool calls.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Get all text from content parts.
    pub fn all_text(&self) -> String {
        content_utils::extract_text(&self.content)
    }

    /// Get the combined reasoning text.
    pub fn reasoning_text(&self) -> String {
        super::reasoning_output::reasoning_text(&self.reasoning)
    }

    /// Parse the output as a specific type.
    pub fn parse_output<T: serde::de::DeserializeOwned>(
        &self,
    ) -> Option<Result<T, serde_json::Error>> {
        self.output
            .as_ref()
            .map(|v| serde_json::from_value(v.clone()))
    }
}

/// A tool call extracted from the response.
#[derive(Debug, Clone)]
pub struct ToolCall {
    /// The tool call ID.
    pub tool_call_id: String,
    /// The tool name.
    pub tool_name: String,
    /// The tool arguments.
    pub args: JSONValue,
    /// Whether this is a dynamic tool call (true) or static (false).
    pub dynamic: bool,
    /// Whether the tool was executed by the provider.
    pub provider_executed: bool,
    /// Whether the tool call arguments are invalid (unparsable).
    pub invalid: bool,
    /// The error that caused the tool call to be invalid.
    pub error: Option<String>,
    /// Display title for the tool call.
    pub title: Option<String>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
}

impl ToolCall {
    /// Create a new tool call.
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        args: JSONValue,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            args,
            dynamic: false,
            provider_executed: false,
            invalid: false,
            error: None,
            title: None,
            provider_metadata: None,
        }
    }

    /// Parse the arguments as a specific type.
    pub fn parse_args<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.args.clone())
    }
}

/// Static tool call (tools known at compile time).
pub type StaticToolCall = ToolCall;
/// Dynamic tool call (dynamically dispatched).
pub type DynamicToolCall = ToolCall;
/// Union type matching TS TypedToolCall.
pub type TypedToolCall = ToolCall;

/// A tool result from an executed tool.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// The tool call ID this result is for.
    pub tool_call_id: String,
    /// The tool name.
    pub tool_name: String,
    /// The result.
    pub result: JSONValue,
    /// Whether the result is an error.
    pub is_error: bool,
    /// The tool input arguments.
    pub input: Option<JSONValue>,
    /// Whether this is a dynamic tool result.
    pub dynamic: bool,
    /// Whether the tool was executed by the provider.
    pub provider_executed: bool,
    /// Whether this is a preliminary (streaming partial) result.
    pub preliminary: bool,
    /// Display title for the tool result.
    pub title: Option<String>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
}

impl ToolResult {
    /// Create a new tool result.
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        result: JSONValue,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            result,
            is_error: false,
            input: None,
            dynamic: false,
            provider_executed: false,
            preliminary: false,
            title: None,
            provider_metadata: None,
        }
    }

    /// Create an error tool result.
    pub fn error(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            result: serde_json::json!({ "error": error.into() }),
            is_error: true,
            input: None,
            dynamic: false,
            provider_executed: false,
            preliminary: false,
            title: None,
            provider_metadata: None,
        }
    }

    /// Parse the result as a specific type.
    pub fn parse<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.result.clone())
    }
}

/// Static tool result (tools known at compile time).
pub type StaticToolResult = ToolResult;
/// Dynamic tool result (dynamically dispatched).
pub type DynamicToolResult = ToolResult;
/// Union type matching TS TypedToolResult.
pub type TypedToolResult = ToolResult;

#[cfg(test)]
#[path = "generate_text_result.test.rs"]
mod tests;
