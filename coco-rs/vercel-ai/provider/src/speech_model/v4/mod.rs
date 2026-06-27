//! Speech model trait and related types (V4).
//!
//! This module defines the `SpeechModelV4` trait for implementing text-to-speech models
//! that follow the Vercel AI SDK v4 specification.

use async_trait::async_trait;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

use crate::errors::AISdkError;
use crate::shared::ProviderMetadata;
use crate::shared::ProviderOptions;
use crate::shared::Warning;

/// The speech model trait (V4).
///
/// This trait defines the interface for text-to-speech models following the
/// Vercel AI SDK v4 specification.
#[async_trait]
pub trait SpeechModelV4: Send + Sync {
    /// Get the specification version.
    fn specification_version(&self) -> &'static str {
        "v4"
    }

    /// Get the provider name.
    fn provider(&self) -> &str;

    /// Get the model ID.
    fn model_id(&self) -> &str;

    /// Generate speech from text.
    async fn do_generate_speech(
        &self,
        options: SpeechModelV4CallOptions,
    ) -> Result<SpeechModelV4Result, AISdkError>;
}

/// Options for a speech model call.
#[derive(Debug, Clone, Default)]
pub struct SpeechModelV4CallOptions {
    /// The text to convert to speech.
    pub text: String,
    /// The voice to use (plain string, provider-specific identifier).
    pub voice: Option<String>,
    /// The output format (plain string, provider-specific identifier).
    pub output_format: Option<String>,
    /// The speed of the speech (0.25 to 4.0).
    pub speed: Option<f32>,
    /// Instructions for the speech generation.
    pub instructions: Option<String>,
    /// The language for speech generation.
    pub language: Option<String>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
    /// Headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
}

impl SpeechModelV4CallOptions {
    /// Create new call options.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    /// Set the voice.
    pub fn with_voice(mut self, voice: impl Into<String>) -> Self {
        self.voice = Some(voice.into());
        self
    }

    /// Set the output format.
    pub fn with_output_format(mut self, format: impl Into<String>) -> Self {
        self.output_format = Some(format.into());
        self
    }

    /// Set the speed.
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = Some(speed);
        self
    }

    /// Set the instructions.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Set the language.
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
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

/// The result of a speech generation call.
#[derive(Debug, Clone)]
pub struct SpeechModelV4Result {
    /// The audio data.
    pub audio: Vec<u8>,
    /// The MIME type of the audio.
    pub content_type: String,
    /// Warnings from the provider.
    pub warnings: Vec<Warning>,
    /// Response metadata.
    pub response: SpeechModelV4Response,
    /// Request metadata.
    pub request: Option<SpeechModelV4Request>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
}

impl SpeechModelV4Result {
    /// Create a new speech result.
    pub fn new(audio: Vec<u8>, content_type: impl Into<String>) -> Self {
        Self {
            audio,
            content_type: content_type.into(),
            warnings: Vec::new(),
            response: SpeechModelV4Response::default(),
            request: None,
            provider_metadata: None,
        }
    }

    /// Create from MP3 data.
    pub fn mp3(audio: Vec<u8>) -> Self {
        Self::new(audio, "audio/mpeg")
    }

    /// Create from WAV data.
    pub fn wav(audio: Vec<u8>) -> Self {
        Self::new(audio, "audio/wav")
    }

    /// Set warnings.
    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = warnings;
        self
    }

    /// Set response metadata.
    pub fn with_response(mut self, response: SpeechModelV4Response) -> Self {
        self.response = response;
        self
    }

    /// Set request metadata.
    pub fn with_request(mut self, request: SpeechModelV4Request) -> Self {
        self.request = Some(request);
        self
    }

    /// Set provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// Response metadata from a speech generation call.
#[derive(Debug, Clone, Default)]
pub struct SpeechModelV4Response {
    /// The timestamp of the response.
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// The model ID used.
    pub model_id: Option<String>,
    /// Response headers.
    pub headers: Option<HashMap<String, String>>,
    /// The raw response body, if available.
    pub body: Option<serde_json::Value>,
}

impl SpeechModelV4Response {
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

/// Request metadata from a speech generation call.
#[derive(Debug, Clone, Default)]
pub struct SpeechModelV4Request {
    /// The raw request body, if available.
    pub body: Option<serde_json::Value>,
}

impl SpeechModelV4Request {
    /// Set the request body.
    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}

#[cfg(test)]
#[path = "speech_model_v4.test.rs"]
mod tests;
