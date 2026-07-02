//! Backend selection — the retrieval `create_reranker` recipe.
//!
//! Call sites hold the resulting `Arc<dyn VoiceEngine>` and never branch on the
//! concrete backend. Requesting `Local` without the `local-voice` feature
//! compiled yields a typed [`VoiceError::FeatureNotEnabled`] (never a panic or
//! a silent fallback), which `/voice` renders as a rebuild-or-switch hint.

use std::sync::Arc;

use coco_config::VoiceBackend;
use coco_config::VoiceConfig;
use coco_inference::TranscriptionModelV4;

use crate::engine::VoiceEngine;
use crate::error::VoiceError;
use crate::remote::RemoteOpenAiEngine;

/// Build the configured voice engine.
///
/// `remote_handle` is the pre-resolved OpenAI-wire transcription model, injected
/// by app/cli bootstrap (so provider/auth concerns stay in the provider crate).
/// It is required for the `Openai` backend and ignored for `Local`.
pub fn create_voice_engine(
    config: &VoiceConfig,
    remote_handle: Option<Arc<dyn TranscriptionModelV4>>,
) -> Result<Arc<dyn VoiceEngine>, VoiceError> {
    match config.backend {
        VoiceBackend::Openai => {
            let handle = remote_handle.ok_or_else(|| {
                VoiceError::TranscriptionFailed(
                    "no OpenAI transcription model available (missing OpenAI credentials)"
                        .to_string(),
                )
            })?;
            Ok(Arc::new(RemoteOpenAiEngine::new(handle)))
        }
        VoiceBackend::Local => {
            #[cfg(feature = "local-voice")]
            {
                let engine = crate::local::LocalWhisperEngine::try_new(&config.local)?;
                Ok(Arc::new(engine))
            }
            #[cfg(not(feature = "local-voice"))]
            {
                let _ = config;
                Err(VoiceError::FeatureNotEnabled("local"))
            }
        }
    }
}
