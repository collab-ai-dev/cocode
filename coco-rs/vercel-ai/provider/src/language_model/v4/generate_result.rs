//! Language model generate result (V4).

use std::collections::HashMap;

use super::finish_reason::FinishReason;
use super::usage::Usage;
use crate::content::AssistantContentPart;
use crate::shared::ProviderMetadata;
use crate::shared::Warning;

/// The result of a generate call.
#[derive(Debug, Clone)]
pub struct LanguageModelV4GenerateResult {
    /// The generated content parts.
    pub content: Vec<AssistantContentPart>,
    /// Token usage.
    pub usage: Usage,
    /// The finish reason.
    pub finish_reason: FinishReason,
    /// Warnings from the provider.
    pub warnings: Vec<Warning>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
    /// Request information (for telemetry).
    pub request: Option<LanguageModelV4Request>,
    /// Response information.
    pub response: Option<LanguageModelV4Response>,
}

/// Request information for telemetry.
#[derive(Debug, Clone)]
pub struct LanguageModelV4Request {
    /// The request body as JSON.
    pub body: Option<serde_json::Value>,
}

/// Response information.
///
/// Response information — `id`, `timestamp`, `modelId` from the spec metadata
/// flattened in, plus optional headers and body for telemetry.
#[derive(Debug, Clone)]
pub struct LanguageModelV4Response {
    /// ID for the generated response, if the provider sends one.
    pub id: Option<String>,
    /// The timestamp of the response, parsed from the provider.
    /// Stored as a typed `DateTime<Utc>` — providers converting from
    /// Unix seconds or ISO 8601 should land here.
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// The model ID used for the response.
    pub model_id: Option<String>,
    /// Response headers.
    pub headers: Option<HashMap<String, String>>,
    /// The response body as JSON (for debugging).
    pub body: Option<serde_json::Value>,
}

impl LanguageModelV4GenerateResult {
    /// Create a new generate result.
    pub fn new(
        content: Vec<AssistantContentPart>,
        usage: Usage,
        finish_reason: FinishReason,
    ) -> Self {
        Self {
            content,
            usage,
            finish_reason,
            warnings: Vec::new(),
            provider_metadata: None,
            request: None,
            response: None,
        }
    }

    /// Create a simple text result.
    pub fn text(text: impl Into<String>, usage: Usage) -> Self {
        Self::new(
            vec![AssistantContentPart::text(text)],
            usage,
            FinishReason::end_turn(),
        )
    }

    /// Add warnings.
    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = warnings;
        self
    }

    /// Add provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }

    /// Add request information.
    pub fn with_request(mut self, request: LanguageModelV4Request) -> Self {
        self.request = Some(request);
        self
    }

    /// Add response information.
    pub fn with_response(mut self, response: LanguageModelV4Response) -> Self {
        self.response = Some(response);
        self
    }

    /// Get the text content if there's only text.
    pub fn text_content(&self) -> Option<String> {
        if self.content.len() == 1
            && let AssistantContentPart::Text(part) = &self.content[0]
        {
            return Some(part.text.clone());
        }
        None
    }
}

impl LanguageModelV4Request {
    /// Create a new request info.
    pub fn new() -> Self {
        Self { body: None }
    }

    /// Create with body.
    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}

impl Default for LanguageModelV4Request {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageModelV4Response {
    /// Create a new response info.
    pub fn new() -> Self {
        Self {
            id: None,
            timestamp: None,
            model_id: None,
            headers: None,
            body: None,
        }
    }

    /// Set the response ID.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the timestamp.
    pub fn with_timestamp(mut self, timestamp: chrono::DateTime<chrono::Utc>) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    /// Set the model ID.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = Some(model_id.into());
        self
    }

    /// Set the headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set the body.
    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}

impl Default for LanguageModelV4Response {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "generate_result.test.rs"]
mod tests;
