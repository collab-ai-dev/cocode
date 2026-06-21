//! Step result type for generate_text.
//!
//! This module provides the canonical `StepResult` type which represents the
//! result of a single step in a multi-step generation process (e.g., tool calling).

use std::collections::HashMap;

use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FinishReason;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::Source;
use vercel_ai_provider::Usage;
use vercel_ai_provider::Warning;

use super::callback::CallbackModelInfo;
use super::content_utils;
use super::generate_text_result::ToolCall;
use super::generate_text_result::ToolResult;
use super::generated_file::GeneratedFile;
use super::reasoning_output::ReasoningOutput;

use crate::types::LanguageModelRequestMetadata;
use crate::types::LanguageModelResponseMetadata;

/// Result of a single step in multi-step generation.
///
/// Also serves as `OnStepFinishEvent` (type alias in callback module).
#[derive(Debug, Clone)]
pub struct StepResult {
    /// The step number (0-indexed).
    pub step: u32,
    /// Unique call ID for this generation session.
    pub call_id: String,
    /// Model information (provider + model ID).
    pub model: CallbackModelInfo,
    /// The text generated in this step.
    pub text: String,
    /// The content parts from this step.
    pub content: Vec<AssistantContentPart>,
    /// Structured reasoning outputs for this step.
    pub reasoning: Vec<ReasoningOutput>,
    /// Tool calls made in this step.
    pub tool_calls: Vec<ToolCall>,
    /// Tool results from this step.
    pub tool_results: Vec<ToolResult>,
    /// Usage for this step.
    pub usage: Usage,
    /// Finish reason for this step.
    pub finish_reason: FinishReason,
    /// Warnings for this step.
    pub warnings: Vec<Warning>,
    /// Whether this step had an error.
    pub is_error: bool,
    /// Error message if this step failed.
    pub error_message: Option<String>,
    /// Sources referenced in this step.
    pub sources: Vec<Source>,
    /// Files generated in this step.
    pub files: Vec<GeneratedFile>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
    /// Request metadata.
    pub request: Option<LanguageModelRequestMetadata>,
    /// Response metadata.
    pub response: Option<LanguageModelResponseMetadata>,
    /// Telemetry function ID.
    pub function_id: Option<String>,
    /// Telemetry metadata.
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    /// Raw finish reason string from the provider.
    pub raw_finish_reason: Option<String>,
    /// Experimental context from the provider.
    pub experimental_context: Option<serde_json::Value>,
}

impl StepResult {
    /// Create a new step result.
    pub fn new(step: u32, text: String, usage: Usage, finish_reason: FinishReason) -> Self {
        Self {
            step,
            call_id: String::new(),
            model: CallbackModelInfo::new("", ""),
            text,
            content: Vec::new(),
            reasoning: Vec::new(),
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            usage,
            finish_reason,
            warnings: Vec::new(),
            is_error: false,
            error_message: None,
            sources: Vec::new(),
            files: Vec::new(),
            provider_metadata: None,
            request: None,
            response: None,
            function_id: None,
            metadata: None,
            raw_finish_reason: None,
            experimental_context: None,
        }
    }

    /// Create a step result from content parts.
    pub fn from_content(
        step: u32,
        content: Vec<AssistantContentPart>,
        usage: Usage,
        finish_reason: FinishReason,
    ) -> Self {
        let text = content_utils::extract_text(&content);
        let reasoning = content_utils::extract_reasoning_outputs(&content);
        Self {
            step,
            call_id: String::new(),
            model: CallbackModelInfo::new("", ""),
            text,
            reasoning,
            content,
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            usage,
            finish_reason,
            warnings: Vec::new(),
            is_error: false,
            error_message: None,
            sources: Vec::new(),
            files: Vec::new(),
            provider_metadata: None,
            request: None,
            response: None,
            function_id: None,
            metadata: None,
            raw_finish_reason: None,
            experimental_context: None,
        }
    }

    /// Create an error step result.
    pub fn error(step: u32, error_message: impl Into<String>) -> Self {
        Self {
            step,
            call_id: String::new(),
            model: CallbackModelInfo::new("", ""),
            text: String::new(),
            content: Vec::new(),
            reasoning: Vec::new(),
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            usage: Usage::default(),
            finish_reason: FinishReason::error(),
            warnings: Vec::new(),
            is_error: true,
            error_message: Some(error_message.into()),
            sources: Vec::new(),
            files: Vec::new(),
            provider_metadata: None,
            request: None,
            response: None,
            function_id: None,
            metadata: None,
            raw_finish_reason: None,
            experimental_context: None,
        }
    }

    /// Set the call ID.
    pub fn with_call_id(mut self, call_id: impl Into<String>) -> Self {
        self.call_id = call_id.into();
        self
    }

    /// Set the model info.
    pub fn with_model(mut self, model: CallbackModelInfo) -> Self {
        self.model = model;
        self
    }

    /// Set the model ID (convenience setter).
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model.model_id = model_id.into();
        self
    }

    /// Set the provider ID (convenience setter).
    pub fn with_provider_id(mut self, provider_id: impl Into<String>) -> Self {
        self.model.provider = provider_id.into();
        self
    }

    /// Add tool calls to this step.
    pub fn with_tool_calls(mut self, tool_calls: Vec<ToolCall>) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    /// Add tool results to this step.
    pub fn with_tool_results(mut self, tool_results: Vec<ToolResult>) -> Self {
        self.tool_results = tool_results;
        self
    }

    /// Add warnings to this step.
    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = warnings;
        self
    }

    /// Add content to this step.
    pub fn with_content(mut self, content: Vec<AssistantContentPart>) -> Self {
        self.text = content_utils::extract_text(&content);
        self.reasoning = content_utils::extract_reasoning_outputs(&content);
        self.content = content;
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

    /// Add provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
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

    /// Set the telemetry function ID.
    pub fn with_function_id(mut self, function_id: impl Into<String>) -> Self {
        self.function_id = Some(function_id.into());
        self
    }

    /// Set the telemetry metadata.
    pub fn with_metadata(mut self, metadata: HashMap<String, serde_json::Value>) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Set the raw finish reason.
    pub fn with_raw_finish_reason(mut self, raw: impl Into<String>) -> Self {
        self.raw_finish_reason = Some(raw.into());
        self
    }

    /// Set the experimental context.
    pub fn with_experimental_context(mut self, context: serde_json::Value) -> Self {
        self.experimental_context = Some(context);
        self
    }

    /// Check if this step has tool calls.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Check if this step has tool results.
    pub fn has_tool_results(&self) -> bool {
        !self.tool_results.is_empty()
    }

    /// Check if this step is the final step.
    pub fn is_final(&self) -> bool {
        matches!(
            self.finish_reason.unified,
            vercel_ai_provider::UnifiedFinishReason::EndTurn
                | vercel_ai_provider::UnifiedFinishReason::MaxTokens
                | vercel_ai_provider::UnifiedFinishReason::ContentFilter
        )
    }

    /// Get the message types in this step.
    pub fn message_types(&self) -> Vec<&'static str> {
        let mut types = Vec::new();

        if !self.text.is_empty() {
            types.push("text");
        }
        if self.has_tool_calls() {
            types.push("tool_calls");
        }
        if self.has_tool_results() {
            types.push("tool_results");
        }

        types
    }

    /// Get the number of tokens used in this step.
    pub fn total_tokens(&self) -> u64 {
        self.usage.total_tokens()
    }

    /// Get all text from content parts.
    pub fn all_text(&self) -> String {
        content_utils::extract_text(&self.content)
    }

    /// Get the combined reasoning text.
    pub fn reasoning_text(&self) -> String {
        super::reasoning_output::reasoning_text(&self.reasoning)
    }

    /// Get the model ID (convenience accessor).
    pub fn model_id(&self) -> &str {
        &self.model.model_id
    }

    /// Get the provider ID (convenience accessor).
    pub fn provider_id(&self) -> &str {
        &self.model.provider
    }
}

impl Default for StepResult {
    fn default() -> Self {
        Self::new(0, String::new(), Usage::default(), FinishReason::end_turn())
    }
}

#[cfg(test)]
#[path = "step_result.test.rs"]
mod tests;
