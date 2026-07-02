//! Tier-2 (`thiserror`) errors for the voice subsystem. Standalone crate, so
//! no `coco-error` dep — main-trunk callers (app/tui) convert at the boundary.

/// Failure modes across capture → transcribe → insert.
#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    /// No microphone is available (or capture not compiled in).
    #[error("no microphone / audio device available")]
    NoAudioDevice,

    /// The recording contained no recognizable speech.
    #[error("no speech detected in the recording")]
    NoSpeechDetected,

    /// The selected backend was not compiled into this build (e.g. `local`
    /// requested without the `local-voice` feature).
    #[error("voice backend `{0}` is not available in this build")]
    FeatureNotEnabled(&'static str),

    /// Remote/local transcription returned an error.
    #[error("transcription failed: {0}")]
    TranscriptionFailed(String),

    /// A network/connection error reaching the remote STT service.
    #[error("voice connection failed: {0}")]
    Connection(String),

    /// Recording was cancelled before completion.
    #[error("recording was cancelled")]
    Cancelled,

    /// Underlying audio-capture error.
    #[error(transparent)]
    Capture(#[from] coco_utils_audio::AudioCaptureError),
}
