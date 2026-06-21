//! Result types for generate_object and stream_object.

use vercel_ai_provider::FinishReason;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::Usage;

use crate::generate_text::ReasoningOutput;
use crate::types::LanguageModelRequestMetadata;
use crate::types::LanguageModelResponseMetadata;

/// Event data for the on_finish callback in generate_object.
#[derive(Debug, Clone)]
pub struct GenerateObjectFinishEvent {
    /// Token usage.
    pub usage: Usage,
    /// The finish reason.
    pub finish_reason: FinishReason,
    /// The raw JSON string.
    pub raw: String,
    /// Warnings from the provider.
    pub warnings: Vec<vercel_ai_provider::Warning>,
}

/// Result of `generate_object`.
#[derive(Debug)]
#[must_use]
pub struct GenerateObjectResult<T> {
    /// The generated object.
    pub object: T,
    /// The raw JSON string.
    pub raw: String,
    /// Token usage.
    pub usage: Usage,
    /// The finish reason.
    pub finish_reason: FinishReason,
    /// Structured reasoning outputs (with text, signature, provider metadata).
    pub reasoning: Vec<ReasoningOutput>,
    /// Warnings.
    pub warnings: Vec<vercel_ai_provider::Warning>,
    /// Request metadata (for telemetry).
    pub request: Option<LanguageModelRequestMetadata>,
    /// Response metadata.
    pub response: Option<LanguageModelResponseMetadata>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
}

impl<T> GenerateObjectResult<T> {
    /// Create a new generate object result.
    pub fn new(object: T, raw: String, usage: Usage, finish_reason: FinishReason) -> Self {
        Self {
            object,
            raw,
            usage,
            finish_reason,
            reasoning: Vec::new(),
            warnings: Vec::new(),
            request: None,
            response: None,
            provider_metadata: None,
        }
    }

    /// Add warnings.
    pub fn with_warnings(mut self, warnings: Vec<vercel_ai_provider::Warning>) -> Self {
        self.warnings = warnings;
        self
    }

    /// Set request metadata.
    pub fn with_request(mut self, request: LanguageModelRequestMetadata) -> Self {
        self.request = Some(request);
        self
    }

    /// Set response metadata.
    pub fn with_response(mut self, response: LanguageModelResponseMetadata) -> Self {
        self.response = Some(response);
        self
    }

    /// Set reasoning outputs.
    pub fn with_reasoning(mut self, reasoning: Vec<ReasoningOutput>) -> Self {
        self.reasoning = reasoning;
        self
    }

    /// Set provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}
