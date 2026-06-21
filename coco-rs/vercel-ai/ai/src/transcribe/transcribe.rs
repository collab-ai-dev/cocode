//! Transcribe audio to text.

use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::TranscriptionModelV4;
use vercel_ai_provider::TranscriptionModelV4CallOptions;

use crate::error::AIError;
use crate::logger::LogWarningsOptions;
use crate::logger::log_warnings;
use crate::provider::get_default_provider;
use crate::types::ProviderOptions;
use crate::types::TranscriptionModelResponseMetadata;
use crate::util::retry::RetryConfig;
use crate::util::retry::with_retry;

use super::transcribe_result::TranscriptionResult;
use super::transcribe_result::TranscriptionSegment;

/// A reference to a transcription model.
#[derive(Clone)]
pub enum TranscriptionModel {
    /// A string model ID that will be resolved via the default provider.
    String(String),
    /// A pre-resolved transcription model.
    V4(Arc<dyn TranscriptionModelV4>),
}

impl Default for TranscriptionModel {
    fn default() -> Self {
        Self::String(String::new())
    }
}

impl TranscriptionModel {
    /// Create from a string ID.
    pub fn from_id(id: impl Into<String>) -> Self {
        Self::String(id.into())
    }

    /// Create from a V4 model.
    pub fn from_v4(model: Arc<dyn TranscriptionModelV4>) -> Self {
        Self::V4(model)
    }

    /// Check if this is a string ID.
    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }
}

impl From<String> for TranscriptionModel {
    fn from(id: String) -> Self {
        Self::String(id)
    }
}

impl From<&str> for TranscriptionModel {
    fn from(id: &str) -> Self {
        Self::String(id.to_string())
    }
}

impl From<Arc<dyn TranscriptionModelV4>> for TranscriptionModel {
    fn from(model: Arc<dyn TranscriptionModelV4>) -> Self {
        Self::V4(model)
    }
}

/// Audio data for transcription.
#[derive(Debug, Clone)]
pub enum AudioData {
    /// Raw audio bytes.
    Bytes(Vec<u8>),
    /// URL to an audio file.
    Url(String),
}

impl Default for AudioData {
    fn default() -> Self {
        Self::Bytes(Vec::new())
    }
}

impl AudioData {
    /// Create from bytes.
    pub fn bytes(data: Vec<u8>) -> Self {
        Self::Bytes(data)
    }

    /// Create from a URL.
    pub fn url(url: impl Into<String>) -> Self {
        Self::Url(url.into())
    }
}

impl From<Vec<u8>> for AudioData {
    fn from(data: Vec<u8>) -> Self {
        Self::Bytes(data)
    }
}

impl From<&[u8]> for AudioData {
    fn from(data: &[u8]) -> Self {
        Self::Bytes(data.to_vec())
    }
}

/// Options for `transcribe`.
#[derive(Default)]
pub struct TranscribeOptions {
    /// The transcription model to use.
    pub model: TranscriptionModel,
    /// The audio data to transcribe.
    pub audio: AudioData,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Maximum number of retries. Set to 0 to disable retries.
    pub max_retries: Option<u32>,
    /// Additional headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
}

impl TranscribeOptions {
    /// Create new options with a model and audio data.
    pub fn new(model: impl Into<TranscriptionModel>, audio: AudioData) -> Self {
        Self {
            model: model.into(),
            audio,
            ..Default::default()
        }
    }

    /// Set provider-specific options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set the maximum retries.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = Some(max_retries);
        self
    }

    /// Set headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }
}

/// Resolve a transcription model reference to an actual model instance.
fn resolve_transcription_model(
    model: TranscriptionModel,
) -> Result<Arc<dyn TranscriptionModelV4>, AIError> {
    match model {
        TranscriptionModel::V4(m) => Ok(m),
        TranscriptionModel::String(id) => {
            let provider = get_default_provider().ok_or_else(|| {
                AIError::InvalidArgument(
                    "No default provider set. Call set_default_provider() first or use a TranscriptionModel::V4 variant.".to_string(),
                )
            })?;
            provider
                .transcription_model(&id)
                .map_err(|e| AIError::ProviderError(AISdkError::new(e.to_string())))
        }
    }
}

/// Transcribe audio to text.
///
/// This function converts audio to text using a speech-to-text model.
///
/// # Arguments
///
/// * `options` - The transcription options including model and audio data.
///
/// # Returns
///
/// A `TranscriptionResult` containing the transcribed text and metadata.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::{transcribe, TranscribeOptions, AudioData};
///
/// // From bytes
/// let audio_bytes = std::fs::read("audio.mp3")?;
/// let result = transcribe(TranscribeOptions {
///     model: "whisper-1".into(),
///     audio: AudioData::bytes(audio_bytes),
///     ..Default::default()
/// }).await?;
///
/// println!("Transcription: {}", result.text);
/// ```
pub async fn transcribe(options: TranscribeOptions) -> Result<TranscriptionResult, AIError> {
    let model = resolve_transcription_model(options.model)?;
    let provider = model.provider().to_string();
    let model_id = model.model_id().to_string();

    // Get audio data
    let audio_data = match options.audio {
        AudioData::Bytes(data) => data,
        AudioData::Url(_url) => {
            return Err(AIError::InvalidArgument(
                "URL audio sources are not yet supported. Please download the audio and pass the bytes directly.".to_string(),
            ));
        }
    };

    // Detect content type from audio data
    let media_type = detect_audio_content_type(&audio_data);

    // Build call options
    let mut call_options = TranscriptionModelV4CallOptions::new(audio_data, media_type);

    if let Some(provider_opts) = options.provider_options {
        call_options = call_options.with_provider_options(provider_opts);
    }
    if let Some(signal) = options.abort_signal {
        call_options = call_options.with_abort_signal(signal);
    }
    if let Some(headers) = options.headers {
        call_options = call_options.with_headers(headers);
    }

    // Build retry config
    let retry_config = options
        .max_retries
        .map(|max_retries| RetryConfig::new().with_max_retries(max_retries))
        .unwrap_or_default();

    // Execute with retry
    let model_clone = model.clone();
    let result = with_retry(retry_config, None, || {
        let model = model_clone.clone();
        let call_options = call_options.clone();
        async move {
            model
                .do_transcribe(call_options)
                .await
                .map_err(AIError::from)
        }
    })
    .await?;

    // Log warnings
    log_warnings(&LogWarningsOptions::new(
        result.warnings.clone(),
        &provider,
        &model_id,
    ));

    // Check if text was generated
    if result.text.is_empty() {
        return Err(AIError::NoTranscriptGenerated);
    }

    // Build response metadata
    let response_meta = TranscriptionModelResponseMetadata {
        timestamp: result.response.timestamp,
        model_id: result.response.model_id.clone(),
        headers: result.response.headers.clone().unwrap_or_default(),
        body: result.response.body.clone(),
    };

    // Build the result
    let mut transcription_result = TranscriptionResult::new(result.text)
        .with_warnings(result.warnings)
        .with_responses(vec![response_meta])
        .with_provider_metadata(result.provider_metadata.unwrap_or_default());

    // Set language
    if let Some(language) = result.language {
        transcription_result = transcription_result.with_language(language);
    }

    // Set duration
    if let Some(duration) = result.duration_in_seconds {
        transcription_result = transcription_result.with_duration_in_seconds(duration);
    }

    // Convert segments
    if let Some(segments) = result.segments {
        let converted: Vec<TranscriptionSegment> = segments
            .into_iter()
            .map(|s| TranscriptionSegment::new(s.text, s.start_second, s.end_second))
            .collect();
        transcription_result = transcription_result.with_segments(converted);
    }

    Ok(transcription_result)
}

/// Detect audio content type from magic bytes.
fn detect_audio_content_type(data: &[u8]) -> String {
    // MP3: ID3 tag or MPEG audio frame sync
    if data.len() >= 3 && &data[0..3] == b"ID3" {
        return "audio/mpeg".to_string();
    }
    if data.len() >= 2 && data[0] == 0xFF && (data[1] & 0xE0) == 0xE0 {
        return "audio/mpeg".to_string();
    }

    // WAV: RIFF...WAVE
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
        return "audio/wav".to_string();
    }

    // OGG: OggS
    if data.len() >= 4 && &data[0..4] == b"OggS" {
        return "audio/ogg".to_string();
    }

    // FLAC: fLaC
    if data.len() >= 4 && &data[0..4] == b"fLaC" {
        return "audio/flac".to_string();
    }

    // M4A/MP4: ftyp
    if data.len() >= 8 && &data[4..8] == b"ftyp" {
        return "audio/mp4".to_string();
    }

    // WebM
    if data.len() >= 4 && &data[0..4] == b"\x1a\x45\xdf\xa3" {
        return "audio/webm".to_string();
    }

    // Default to octet-stream for unknown formats
    "application/octet-stream".to_string()
}

#[cfg(test)]
#[path = "transcribe.test.rs"]
mod tests;
