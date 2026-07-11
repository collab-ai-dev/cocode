use serde::Deserialize;
use vercel_ai_provider::ProviderOptions;

/// Provider-specific options for xAI speech (TTS) generation.
///
/// Mirrors `xaiSpeechModelOptionsSchema` from `xai-speech-model-options.ts`,
/// extracted from `provider_options["xai"]`. The upstream schema constrains
/// `sampleRate` / `bitRate` / `optimizeStreamingLatency` to literal sets;
/// here the values pass through untyped and the API validates them.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct XaiSpeechProviderOptions {
    /// Sample rate of the generated audio in Hz (8000-48000).
    pub sample_rate: Option<i64>,
    /// MP3 bit rate in bits per second. Only applies when the output format
    /// is mp3.
    pub bit_rate: Option<i64>,
    /// Reduce time to first audio chunk (0-2), trading quality for latency.
    pub optimize_streaming_latency: Option<i64>,
    /// Normalize written-form text into spoken-form text before synthesis.
    pub text_normalization: Option<bool>,
}

/// Extract xAI speech options from the generic provider-options map (the
/// `"xai"` namespace). Returns defaults when absent or malformed.
pub fn extract_xai_speech_options(
    provider_options: &Option<ProviderOptions>,
) -> XaiSpeechProviderOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("xai"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<XaiSpeechProviderOptions>(v).ok())
        .unwrap_or_default()
}
