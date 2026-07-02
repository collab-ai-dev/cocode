//! Remote OpenAI-wire STT backend (MVP default).
//!
//! A thin caller of coco's transcription seam
//! (`coco_inference::transcribe_audio`) over an injected model handle — no
//! direct `vercel-ai` dependency (the seam guard forbids it outside
//! `services/inference`). Provider/auth concerns stay in the provider layer;
//! this engine only carries a pre-built handle.

use std::sync::Arc;

use async_trait::async_trait;
use coco_inference::transcribe_audio;
use coco_inference::TranscriptionModelV4;
use tokio_util::sync::CancellationToken;

use crate::engine::TranscribeParams;
use crate::engine::Transcript;
use crate::engine::VoiceCapabilities;
use crate::engine::VoiceEngine;
use crate::error::VoiceError;

/// Batch transcription over an injected OpenAI-wire transcription model.
pub struct RemoteOpenAiEngine {
    handle: Arc<dyn TranscriptionModelV4>,
}

impl RemoteOpenAiEngine {
    pub fn new(handle: Arc<dyn TranscriptionModelV4>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl VoiceEngine for RemoteOpenAiEngine {
    fn name(&self) -> &str {
        "openai"
    }

    fn capabilities(&self) -> VoiceCapabilities {
        VoiceCapabilities {
            requires_network: true,
            on_device: false,
            streaming: false,
        }
    }

    async fn transcribe(
        &self,
        audio: Vec<u8>,
        params: &TranscribeParams,
        cancel: CancellationToken,
    ) -> Result<Transcript, VoiceError> {
        let output = transcribe_audio(self.handle.clone(), audio, params.language.clone(), cancel)
            .await
            .map_err(|e| VoiceError::TranscriptionFailed(e.to_string()))?;
        let text = output.text.trim().to_string();
        if text.is_empty() {
            return Err(VoiceError::NoSpeechDetected);
        }
        Ok(Transcript {
            text,
            language: output.language,
        })
    }
}
