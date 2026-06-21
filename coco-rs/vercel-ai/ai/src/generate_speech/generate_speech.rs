//! Generate speech from text.

use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::SpeechModelV4;
use vercel_ai_provider::SpeechModelV4CallOptions;

use crate::error::AIError;
use crate::logger::LogWarningsOptions;
use crate::logger::log_warnings;
use crate::provider::get_default_provider;
use crate::types::ProviderOptions;
use crate::types::SpeechModelResponseMetadata;
use crate::util::retry::RetryConfig;
use crate::util::retry::with_retry;

use super::speech_result::GeneratedAudioFile;
use super::speech_result::SpeechResult;

/// A reference to a speech model.
#[derive(Clone)]
pub enum SpeechModel {
    /// A string model ID that will be resolved via the default provider.
    String(String),
    /// A pre-resolved speech model.
    V4(Arc<dyn SpeechModelV4>),
}

impl Default for SpeechModel {
    fn default() -> Self {
        Self::String(String::new())
    }
}

impl SpeechModel {
    /// Create from a string ID.
    pub fn from_id(id: impl Into<String>) -> Self {
        Self::String(id.into())
    }

    /// Create from a V4 model.
    pub fn from_v4(model: Arc<dyn SpeechModelV4>) -> Self {
        Self::V4(model)
    }

    /// Check if this is a string ID.
    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }
}

impl From<String> for SpeechModel {
    fn from(id: String) -> Self {
        Self::String(id)
    }
}

impl From<&str> for SpeechModel {
    fn from(id: &str) -> Self {
        Self::String(id.to_string())
    }
}

impl From<Arc<dyn SpeechModelV4>> for SpeechModel {
    fn from(model: Arc<dyn SpeechModelV4>) -> Self {
        Self::V4(model)
    }
}

/// Options for `generate_speech`.
#[derive(Default)]
pub struct GenerateSpeechOptions {
    /// The speech model to use.
    pub model: SpeechModel,
    /// The text to convert to speech.
    pub text: String,
    /// The voice to use for speech generation (plain string, provider-specific).
    pub voice: Option<String>,
    /// The output format (plain string, provider-specific).
    pub output_format: Option<String>,
    /// The speed of speech (0.25 to 4.0).
    pub speed: Option<f32>,
    /// Instructions for the speech generation.
    pub instructions: Option<String>,
    /// The language for speech generation.
    pub language: Option<String>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Maximum number of retries. Set to 0 to disable retries.
    pub max_retries: Option<u32>,
    /// Additional headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
}

impl GenerateSpeechOptions {
    /// Create new options with a model and text.
    pub fn new(model: impl Into<SpeechModel>, text: impl Into<String>) -> Self {
        Self {
            model: model.into(),
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

/// Resolve a speech model reference to an actual model instance.
fn resolve_speech_model(model: SpeechModel) -> Result<Arc<dyn SpeechModelV4>, AIError> {
    match model {
        SpeechModel::V4(m) => Ok(m),
        SpeechModel::String(id) => {
            let provider = get_default_provider().ok_or_else(|| {
                AIError::InvalidArgument(
                    "No default provider set. Call set_default_provider() first or use a SpeechModel::V4 variant.".to_string(),
                )
            })?;
            provider
                .speech_model(&id)
                .map_err(|e| AIError::ProviderError(AISdkError::new(e.to_string())))
        }
    }
}

/// Generate speech audio from text.
///
/// This function converts text to speech using a speech synthesis model.
///
/// # Arguments
///
/// * `options` - The generation options including model, text, and voice settings.
///
/// # Returns
///
/// A `SpeechResult` containing the audio data and metadata.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::{generate_speech, GenerateSpeechOptions};
///
/// let result = generate_speech(GenerateSpeechOptions {
///     model: "tts-1".into(),
///     text: "Hello, world!".to_string(),
///     voice: Some("alloy".to_string()),
///     ..Default::default()
/// }).await?;
///
/// // Save to file
/// std::fs::write("output.mp3", &result.audio.data)?;
/// ```
pub async fn generate_speech(options: GenerateSpeechOptions) -> Result<SpeechResult, AIError> {
    let model = resolve_speech_model(options.model)?;
    let provider = model.provider().to_string();
    let model_id = model.model_id().to_string();

    // Build call options
    let mut call_options = SpeechModelV4CallOptions::new(&options.text);

    if let Some(voice) = options.voice {
        call_options = call_options.with_voice(voice);
    }
    if let Some(format) = options.output_format {
        call_options = call_options.with_output_format(format);
    }
    if let Some(speed) = options.speed {
        call_options = call_options.with_speed(speed);
    }
    if let Some(instructions) = options.instructions {
        call_options = call_options.with_instructions(instructions);
    }
    if let Some(language) = options.language {
        call_options = call_options.with_language(language);
    }
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
                .do_generate_speech(call_options)
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

    // Check if audio was generated
    if result.audio.is_empty() {
        return Err(AIError::NoSpeechGenerated);
    }

    // Build response metadata
    let response_meta = SpeechModelResponseMetadata {
        timestamp: result.response.timestamp,
        model_id: result.response.model_id.clone(),
        headers: result.response.headers.clone().unwrap_or_default(),
        body: result.response.body.clone(),
    };

    // Build the result
    let audio = GeneratedAudioFile::new(result.audio, result.content_type);
    let speech_result = SpeechResult::new(audio)
        .with_warnings(result.warnings)
        .with_responses(vec![response_meta])
        .with_provider_metadata(result.provider_metadata.unwrap_or_default());

    Ok(speech_result)
}

#[cfg(test)]
#[path = "generate_speech.test.rs"]
mod tests;
