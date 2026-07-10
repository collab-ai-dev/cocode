use serde::Deserialize;
use vercel_ai_provider::ProviderOptions;

/// Provider-specific options for Groq transcription models.
///
/// Mirrors `groqTranscriptionModelOptions`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroqTranscriptionProviderOptions {
    /// The language of the input audio in ISO-639-1 format.
    pub language: Option<String>,
    /// Text to guide the model's style or continue a previous audio segment.
    pub prompt: Option<String>,
    /// Output response format (e.g. `verbose_json`, `text`).
    pub response_format: Option<String>,
    /// The sampling temperature, between 0 and 1.
    pub temperature: Option<f64>,
    /// The timestamp granularities to populate (`word` and/or `segment`).
    pub timestamp_granularities: Option<Vec<String>>,
}

/// Extract Groq transcription options from provider options (`"groq"` namespace).
pub fn extract_transcription_options(
    provider_options: &Option<ProviderOptions>,
) -> GroqTranscriptionProviderOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("groq"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<GroqTranscriptionProviderOptions>(v).ok())
        .unwrap_or_default()
}
