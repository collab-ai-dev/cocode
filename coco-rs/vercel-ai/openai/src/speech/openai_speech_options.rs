use serde::Deserialize;

/// Provider-specific options for OpenAI speech models.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAISpeechProviderOptions {
    /// Instructions for the speech generation (e.g. "Speak in a slow and steady tone").
    /// Does not work with tts-1 or tts-1-hd.
    pub instructions: Option<String>,
    /// The speed of the generated audio. Select a value from 0.25 to 4.0.
    /// Defaults to 1.0.
    pub speed: Option<f64>,
}

/// Extract speech-specific options from provider options.
pub fn extract_speech_options(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
) -> OpenAISpeechProviderOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("openai"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<OpenAISpeechProviderOptions>(v).ok())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "openai_speech_options.test.rs"]
mod tests;
