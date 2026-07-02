//! Tier-2 (`thiserror`) errors for audio capture. Main-trunk callers convert
//! at the boundary via `coco_error::boxed(err, StatusCode::X)`.

/// Failure modes when probing for, starting, or finalizing a recording.
#[derive(Debug, thiserror::Error)]
pub enum AudioCaptureError {
    /// Microphone capture was not compiled in (the `cpal` feature is off).
    #[error("audio capture is not compiled into this build (enable the `cpal` feature)")]
    NotCompiled,

    /// No default input device is present (no microphone).
    #[error("no microphone / default input device available")]
    NoInputDevice,

    /// The device's stream format is not one we can decode.
    #[error("unsupported input sample format: {0}")]
    UnsupportedFormat(String),

    /// The OS denied microphone access, or capture produced nothing.
    #[error("no audio was captured from the microphone")]
    NoAudioCaptured,

    /// Underlying backend (cpal) error building or running the stream.
    #[error("audio backend error: {0}")]
    Backend(String),

    /// WAV encoding of the captured samples failed.
    #[error("failed to encode captured audio as WAV: {0}")]
    Encode(String),
}
