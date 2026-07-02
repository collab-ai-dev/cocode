//! The backend-agnostic transcription seam.
//!
//! Local and remote STT both implement [`VoiceEngine`]; call sites hold an
//! `Arc<dyn VoiceEngine>` and branch on [`VoiceCapabilities`], never on backend
//! identity (mirrors retrieval's `Reranker` / `RerankerCapabilities`).

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::VoiceError;

/// Self-describing properties of an engine, so callers can render the right
/// privacy posture ("on-device" vs "via OpenAI") without knowing the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceCapabilities {
    /// Audio is sent to a remote service (privacy/cost-sensitive).
    pub requires_network: bool,
    /// Transcription runs on-device.
    pub on_device: bool,
    /// Partial transcripts are streamed (vs one-shot batch).
    pub streaming: bool,
}

/// Parameters for a single transcription request.
#[derive(Debug, Clone, Default)]
pub struct TranscribeParams {
    /// BCP-47 / ISO-639-1 language, or `None` to auto-detect (config `"auto"`).
    pub language: Option<String>,
}

/// A finished transcription.
#[derive(Debug, Clone)]
pub struct Transcript {
    /// The recognized text (already trimmed).
    pub text: String,
    /// Detected / used language, when the backend reports it.
    pub language: Option<String>,
}

/// One speech-to-text backend.
#[async_trait]
pub trait VoiceEngine: Send + Sync {
    /// Stable identifier for status/footer text (e.g. `"openai"`, `"local"`).
    fn name(&self) -> &str;

    /// Static capabilities of this backend.
    fn capabilities(&self) -> VoiceCapabilities;

    /// Transcribe a finished 16 kHz mono WAV buffer. `cancel` aborts an
    /// in-flight request.
    async fn transcribe(
        &self,
        audio: Vec<u8>,
        params: &TranscribeParams,
        cancel: CancellationToken,
    ) -> Result<Transcript, VoiceError>;
}
