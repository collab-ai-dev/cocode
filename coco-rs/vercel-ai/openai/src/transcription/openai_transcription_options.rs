use serde::Deserialize;

/// Provider-specific options for OpenAI transcription models.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAITranscriptionProviderOptions {
    /// Additional information to include in the transcription response.
    pub include: Option<Vec<String>>,
    /// The language of the input audio in ISO-639-1 format.
    pub language: Option<String>,
    /// An optional text to guide the model's style or continue a previous audio segment.
    pub prompt: Option<String>,
    /// The sampling temperature, between 0 and 1.
    pub temperature: Option<f64>,
    /// The timestamp granularities to populate for this transcription.
    /// Defaults to `["segment"]`.
    pub timestamp_granularities: Option<Vec<String>>,
}

/// Extract transcription-specific options from provider options.
pub fn extract_transcription_options(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
) -> OpenAITranscriptionProviderOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("openai"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<OpenAITranscriptionProviderOptions>(v).ok())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "openai_transcription_options.test.rs"]
mod tests;
