//! Speech result types.

use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::Warning;

use crate::types::SpeechModelResponseMetadata;

/// Result of a `generate_speech` call.
#[derive(Debug)]
#[must_use]
pub struct SpeechResult {
    /// The generated audio file.
    pub audio: GeneratedAudioFile,
    /// Warnings from the provider.
    pub warnings: Vec<Warning>,
    /// Response metadata from the provider.
    pub responses: Vec<SpeechModelResponseMetadata>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
}

impl SpeechResult {
    /// Create a new speech result.
    pub fn new(audio: GeneratedAudioFile) -> Self {
        Self {
            audio,
            warnings: Vec::new(),
            responses: Vec::new(),
            provider_metadata: None,
        }
    }

    /// Add warnings.
    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = warnings;
        self
    }

    /// Set responses.
    pub fn with_responses(mut self, responses: Vec<SpeechModelResponseMetadata>) -> Self {
        self.responses = responses;
        self
    }

    /// Set provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// A generated audio file.
#[derive(Debug, Clone)]
pub struct GeneratedAudioFile {
    /// The audio data.
    pub data: Vec<u8>,
    /// The MIME type of the audio (e.g., "audio/mpeg", "audio/wav").
    pub media_type: String,
}

impl GeneratedAudioFile {
    /// Create a new audio file.
    pub fn new(data: Vec<u8>, media_type: impl Into<String>) -> Self {
        Self {
            data,
            media_type: media_type.into(),
        }
    }

    /// Create from MP3 data.
    pub fn mp3(data: Vec<u8>) -> Self {
        Self::new(data, "audio/mpeg")
    }

    /// Create from WAV data.
    pub fn wav(data: Vec<u8>) -> Self {
        Self::new(data, "audio/wav")
    }

    /// Create from Opus data.
    pub fn opus(data: Vec<u8>) -> Self {
        Self::new(data, "audio/opus")
    }

    /// Get the file extension based on the media type.
    pub fn extension(&self) -> &str {
        match self.media_type.as_str() {
            "audio/mpeg" | "audio/mp3" => "mp3",
            "audio/wav" | "audio/wave" => "wav",
            "audio/opus" => "opus",
            "audio/aac" => "aac",
            "audio/flac" => "flac",
            "audio/ogg" => "ogg",
            _ => "bin",
        }
    }

    /// Get the base64-encoded data.
    pub fn to_base64(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&self.data)
    }
}
