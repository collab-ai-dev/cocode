use serde::Deserialize;
use vercel_ai_provider::ProviderOptions;

/// Audio encoding for raw, headerless input audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XaiTranscriptionAudioFormat {
    Pcm,
    Mulaw,
    Alaw,
}

impl XaiTranscriptionAudioFormat {
    /// Wire value for the `audio_format` form field.
    pub fn as_str(&self) -> &'static str {
        match self {
            XaiTranscriptionAudioFormat::Pcm => "pcm",
            XaiTranscriptionAudioFormat::Mulaw => "mulaw",
            XaiTranscriptionAudioFormat::Alaw => "alaw",
        }
    }
}

/// Terms to bias transcription toward — a single term or a list.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum XaiKeyterm {
    One(String),
    Many(Vec<String>),
}

impl XaiKeyterm {
    /// Flatten into the repeated `keyterm` form-field values.
    pub fn terms(&self) -> Vec<String> {
        match self {
            XaiKeyterm::One(term) => vec![term.clone()],
            XaiKeyterm::Many(terms) => terms.clone(),
        }
    }
}

/// Streaming-only options (WebSocket STT), the `streaming` sub-object.
///
/// Mirrors the `streaming` schema in `xai-transcription-model-options.ts`.
/// These flow into the WebSocket URL query string, not the batch multipart
/// form.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct XaiStreamingOptions {
    /// Emit interim transcript results while speech is being processed.
    pub interim_results: Option<bool>,
    /// Silence duration in ms before an utterance-final event.
    pub endpointing: Option<i64>,
    /// End-of-turn detection threshold (0-1). When set, enables Smart Turn.
    pub smart_turn: Option<f64>,
    /// Maximum silence in ms before forcing `speech_final`.
    pub smart_turn_timeout: Option<i64>,
}

/// Provider-specific options for xAI transcription (STT), shared by the batch
/// (`do_transcribe`) and streaming (`do_stream`) paths.
///
/// Mirrors `xaiTranscriptionModelOptionsSchema` from
/// `xai-transcription-model-options.ts`, extracted from `provider_options["xai"]`.
/// The `streaming` sub-object configures the WebSocket STT path only.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct XaiTranscriptionProviderOptions {
    /// Audio encoding for raw, headerless input audio.
    pub audio_format: Option<XaiTranscriptionAudioFormat>,
    /// Sample rate of the input audio in Hz.
    pub sample_rate: Option<i64>,
    /// Language code used for inverse text normalization.
    pub language: Option<String>,
    /// Enable inverse text normalization. Requires `language`.
    pub format: Option<bool>,
    /// Enable per-channel transcription for multichannel audio.
    pub multichannel: Option<bool>,
    /// Number of interleaved audio channels (2-8).
    pub channels: Option<i64>,
    /// Enable speaker diarization.
    pub diarize: Option<bool>,
    /// Terms to bias transcription toward.
    pub keyterm: Option<XaiKeyterm>,
    /// Include filler words such as "uh" and "um" in the transcript.
    pub filler_words: Option<bool>,
    /// Options for streaming speech-to-text over WebSocket.
    pub streaming: Option<XaiStreamingOptions>,
}

/// Extract xAI transcription options from the generic provider-options map
/// (the `"xai"` namespace). Returns defaults when absent or malformed.
pub fn extract_xai_transcription_options(
    provider_options: &Option<ProviderOptions>,
) -> XaiTranscriptionProviderOptions {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("xai"))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value::<XaiTranscriptionProviderOptions>(v).ok())
        .unwrap_or_default()
}
