//! Streaming transcription (V4) — the real-time speech-to-text surface.
//!
//! Mirrors `@ai-sdk/provider`'s `TranscriptionModelV4` `doStream` types:
//! callers push audio chunks through [`TranscriptionModelV4StreamOptions::audio`]
//! and receive an ordered stream of [`TranscriptionModelV4StreamPart`]s
//! (partial / final transcript segments, a terminal `finish`). Implemented by
//! WebSocket-based providers (e.g. xAI `/stt`).

use std::collections::HashMap;
use std::pin::Pin;

use chrono::DateTime;
use chrono::Utc;
use futures::Stream;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::JSONValue;
use crate::errors::AISdkError;
use crate::shared::ProviderMetadata;
use crate::shared::ProviderOptions;
use crate::shared::Warning;

use super::TranscriptionSegmentV4;

/// The input audio format for the raw audio chunks pushed into a streaming
/// transcription call.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptionInputAudioFormat {
    /// Audio format media type, e.g. `audio/pcm`, `audio/pcmu`, or `audio/pcma`.
    pub media_type: String,
    /// Sample rate in Hz. Only applicable for formats that require a rate.
    pub rate: Option<i64>,
}

impl TranscriptionInputAudioFormat {
    /// Create a new input audio format.
    pub fn new(media_type: impl Into<String>, rate: Option<i64>) -> Self {
        Self {
            media_type: media_type.into(),
            rate,
        }
    }
}

/// A stream of audio byte chunks fed into a streaming transcription call.
///
/// Unlike the TS spec (`ReadableStream<Uint8Array | string>`, where `string`
/// carries base64), coco-rs accepts raw bytes only; callers decode base64
/// before pushing.
pub type AudioChunkStream = Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>;

/// Options for a streaming transcription call.
///
/// Not `Clone`/`Debug` because [`Self::audio`] is a live stream — mirrors
/// `LanguageModelV4StreamResult`, which is likewise move-only.
pub struct TranscriptionModelV4StreamOptions {
    /// Audio chunks to transcribe (raw bytes).
    pub audio: AudioChunkStream,
    /// The input audio format for the raw audio chunks.
    pub input_audio_format: TranscriptionInputAudioFormat,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
    /// Additional HTTP/WebSocket headers.
    pub headers: Option<HashMap<String, String>>,
    /// When true, providers include raw provider chunks in the stream.
    pub include_raw_chunks: bool,
}

/// A single part of a streaming transcription response.
///
/// Serializes with the spec's kebab-case `type` discriminant (`stream-start`,
/// `transcript-partial`, …) so test harnesses can snapshot the decoded shape.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum TranscriptionModelV4StreamPart {
    /// Stream start with warnings for the call (e.g. unsupported settings).
    StreamStart {
        /// Warnings surfaced before any transcript.
        warnings: Vec<Warning>,
    },
    /// Append-only transcript delta.
    TranscriptDelta {
        /// Optional block id (e.g. a channel id for multichannel audio).
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// The appended text.
        delta: String,
        /// Provider-specific metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Non-final transcript text. May be revised by later parts.
    TranscriptPartial {
        /// Optional block id.
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// The interim text.
        text: String,
        /// Start offset in seconds.
        #[serde(skip_serializing_if = "Option::is_none")]
        start_second: Option<f64>,
        /// Duration in seconds.
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_in_seconds: Option<f64>,
        /// Channel index for multichannel audio.
        #[serde(skip_serializing_if = "Option::is_none")]
        channel_index: Option<i64>,
        /// Provider-specific metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Final transcript text for a provider-defined segment or utterance.
    TranscriptFinal {
        /// Optional block id.
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// The final text.
        text: String,
        /// Start offset in seconds.
        #[serde(skip_serializing_if = "Option::is_none")]
        start_second: Option<f64>,
        /// End offset in seconds.
        #[serde(skip_serializing_if = "Option::is_none")]
        end_second: Option<f64>,
        /// Channel index for multichannel audio.
        #[serde(skip_serializing_if = "Option::is_none")]
        channel_index: Option<i64>,
        /// Provider-specific metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Response metadata, emitted once available.
    ResponseMetadata {
        /// The response timestamp.
        #[serde(skip_serializing_if = "Option::is_none")]
        timestamp: Option<DateTime<Utc>>,
        /// The model id used.
        #[serde(skip_serializing_if = "Option::is_none")]
        model_id: Option<String>,
        /// Response headers.
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<HashMap<String, String>>,
        /// The raw response body, if available.
        #[serde(skip_serializing_if = "Option::is_none")]
        body: Option<JSONValue>,
    },
    /// Terminal metadata, emitted after the transcript is finished.
    Finish {
        /// The full concatenated transcript.
        text: String,
        /// Provider-defined segments (may be empty).
        segments: Vec<TranscriptionSegmentV4>,
        /// The detected language.
        #[serde(skip_serializing_if = "Option::is_none")]
        language: Option<String>,
        /// The audio duration in seconds.
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_in_seconds: Option<f64>,
        /// Provider-specific metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// A raw provider chunk (only when `include_raw_chunks` is set).
    Raw {
        /// The raw provider event.
        raw_value: JSONValue,
    },
    /// A streamed error. Multiple may be emitted.
    Error {
        /// The error message.
        error: String,
    },
}

/// The result of a streaming transcription call.
///
/// Not `Clone`/`Debug`: [`Self::stream`] is a live stream (mirrors
/// `LanguageModelV4StreamResult`).
pub struct TranscriptionModelV4StreamResult {
    /// The ordered stream of transcript parts.
    pub stream:
        Pin<Box<dyn Stream<Item = Result<TranscriptionModelV4StreamPart, AISdkError>> + Send>>,
    /// Request metadata (e.g. the WebSocket URL).
    pub request: Option<super::TranscriptionModelV4Request>,
    /// Response metadata.
    pub response: super::TranscriptionModelV4Response,
}

#[cfg(test)]
#[path = "stream.test.rs"]
mod tests;
