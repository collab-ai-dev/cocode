//! Transcription model trait and related types (V4).
//!
//! This module defines the `TranscriptionModelV4` trait for implementing speech-to-text models
//! that follow the Vercel AI SDK v4 specification.

use async_trait::async_trait;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

use crate::errors::AISdkError;
use crate::shared::ProviderMetadata;
use crate::shared::ProviderOptions;
use crate::shared::Warning;

/// The transcription model trait (V4).
///
/// This trait defines the interface for speech-to-text models following the
/// Vercel AI SDK v4 specification.
#[async_trait]
pub trait TranscriptionModelV4: Send + Sync {
    /// Get the specification version.
    fn specification_version(&self) -> &'static str {
        "v4"
    }

    /// Get the provider name.
    fn provider(&self) -> &str;

    /// Get the model ID.
    fn model_id(&self) -> &str;

    /// Transcribe audio to text.
    async fn do_transcribe(
        &self,
        options: TranscriptionModelV4CallOptions,
    ) -> Result<TranscriptionModelV4Result, AISdkError>;
}

/// Options for a transcription model call.
#[derive(Debug, Clone, Default)]
pub struct TranscriptionModelV4CallOptions {
    /// The audio data to transcribe.
    pub audio: Vec<u8>,
    /// The MIME type of the audio.
    pub media_type: String,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
    /// Headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
}

impl TranscriptionModelV4CallOptions {
    /// Create new call options.
    pub fn new(audio: Vec<u8>, media_type: impl Into<String>) -> Self {
        Self {
            audio,
            media_type: media_type.into(),
            ..Default::default()
        }
    }

    /// Set provider options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Set headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }
}

/// The result of a transcription call.
#[derive(Debug, Clone)]
pub struct TranscriptionModelV4Result {
    /// The transcribed text.
    pub text: String,
    /// The detected language.
    pub language: Option<String>,
    /// Duration of the audio in seconds.
    pub duration_in_seconds: Option<f64>,
    /// Segment-level timestamps.
    pub segments: Option<Vec<TranscriptionSegmentV4>>,
    /// Warnings from the provider.
    pub warnings: Vec<Warning>,
    /// Response metadata.
    pub response: TranscriptionModelV4Response,
    /// Request metadata.
    pub request: Option<TranscriptionModelV4Request>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
}

impl TranscriptionModelV4Result {
    /// Create a new transcription result.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            language: None,
            duration_in_seconds: None,
            segments: None,
            warnings: Vec::new(),
            response: TranscriptionModelV4Response::default(),
            request: None,
            provider_metadata: None,
        }
    }

    /// Set the language.
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Set the duration in seconds.
    pub fn with_duration_in_seconds(mut self, duration: f64) -> Self {
        self.duration_in_seconds = Some(duration);
        self
    }

    /// Set the segments.
    pub fn with_segments(mut self, segments: Vec<TranscriptionSegmentV4>) -> Self {
        self.segments = Some(segments);
        self
    }

    /// Set warnings.
    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = warnings;
        self
    }

    /// Set response metadata.
    pub fn with_response(mut self, response: TranscriptionModelV4Response) -> Self {
        self.response = response;
        self
    }

    /// Set request metadata.
    pub fn with_request(mut self, request: TranscriptionModelV4Request) -> Self {
        self.request = Some(request);
        self
    }

    /// Set provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// A transcription segment with timing information (V4 spec).
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptionSegmentV4 {
    /// The segment text.
    pub text: String,
    /// Start time in seconds.
    pub start_second: f64,
    /// End time in seconds.
    pub end_second: f64,
}

impl TranscriptionSegmentV4 {
    /// Create a new transcription segment.
    pub fn new(text: impl Into<String>, start_second: f64, end_second: f64) -> Self {
        Self {
            text: text.into(),
            start_second,
            end_second,
        }
    }
}

/// Response metadata from a transcription call.
#[derive(Debug, Clone, Default)]
pub struct TranscriptionModelV4Response {
    /// The timestamp of the response.
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// The model ID used.
    pub model_id: Option<String>,
    /// Response headers.
    pub headers: Option<HashMap<String, String>>,
    /// The raw response body, if available.
    pub body: Option<serde_json::Value>,
}

impl TranscriptionModelV4Response {
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

    /// Set response headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set the response body.
    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}

/// Request metadata from a transcription call.
#[derive(Debug, Clone, Default)]
pub struct TranscriptionModelV4Request {
    /// The raw request body, if available.
    pub body: Option<serde_json::Value>,
}

impl TranscriptionModelV4Request {
    /// Set the request body.
    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}

#[cfg(test)]
#[path = "transcription_model_v4.test.rs"]
mod tests;
