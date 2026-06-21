//! Transcription result types.

use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::Warning;

use crate::types::TranscriptionModelResponseMetadata;

/// Result of a `transcribe` call.
#[derive(Debug)]
#[must_use]
pub struct TranscriptionResult {
    /// The transcribed text.
    pub text: String,
    /// Segments with timing information (if available).
    pub segments: Vec<TranscriptionSegment>,
    /// The detected language (if available).
    pub language: Option<String>,
    /// Duration in seconds (if available).
    pub duration_in_seconds: Option<f64>,
    /// Warnings from the provider.
    pub warnings: Vec<Warning>,
    /// Response metadata from the provider.
    pub responses: Vec<TranscriptionModelResponseMetadata>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
}

impl TranscriptionResult {
    /// Create a new transcription result.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            segments: Vec::new(),
            language: None,
            duration_in_seconds: None,
            warnings: Vec::new(),
            responses: Vec::new(),
            provider_metadata: None,
        }
    }

    /// Add segments.
    pub fn with_segments(mut self, segments: Vec<TranscriptionSegment>) -> Self {
        self.segments = segments;
        self
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

    /// Add warnings.
    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = warnings;
        self
    }

    /// Set responses.
    pub fn with_responses(mut self, responses: Vec<TranscriptionModelResponseMetadata>) -> Self {
        self.responses = responses;
        self
    }

    /// Set provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }

    /// Check if segments are available.
    pub fn has_segments(&self) -> bool {
        !self.segments.is_empty()
    }

    /// Get the word count.
    pub fn word_count(&self) -> usize {
        self.text.split_whitespace().count()
    }
}

/// A transcription segment with timing information.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptionSegment {
    /// The segment text.
    pub text: String,
    /// Start time in seconds.
    pub start_second: f64,
    /// End time in seconds.
    pub end_second: f64,
}

impl TranscriptionSegment {
    /// Create a new segment.
    pub fn new(text: impl Into<String>, start_second: f64, end_second: f64) -> Self {
        Self {
            text: text.into(),
            start_second,
            end_second,
        }
    }

    /// Get the duration in seconds.
    pub fn duration(&self) -> f64 {
        self.end_second - self.start_second
    }
}
