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
use tokio::sync::mpsc;

use crate::engine::VoiceEngine;
use crate::error::VoiceError;
use crate::remote::RemoteOpenAiEngine;
use crate::session::VoiceEvent;

/// Build the configured voice engine.
///
/// `remote_handle` is the pre-resolved OpenAI-wire transcription model, injected
/// by app/cli bootstrap (so provider/auth concerns stay in the provider crate).
/// It is required for the `Remote` backend and ignored for `Local`. `events` is
/// the voice event stream the local backend reports download progress on
/// (ignored by the remote backend).
pub fn create_voice_engine(
    config: &VoiceConfig,
    remote_handle: Option<Arc<dyn TranscriptionModelV4>>,
    events: Option<mpsc::Sender<VoiceEvent>>,
) -> Result<Arc<dyn VoiceEngine>, VoiceError> {
    match config.backend {
        VoiceBackend::Remote => {
            let handle = remote_handle.ok_or_else(|| {
                VoiceError::TranscriptionFailed(format!(
                    "no transcription model available for provider `{}` \
                     (missing or non-OpenAI-wire provider credentials)",
                    config.remote.provider
                ))
            })?;
            Ok(Arc::new(RemoteOpenAiEngine::new(handle)))
        }
        VoiceBackend::Local => create_local_engine(config, events),
    }
}

/// Dispatch the on-device backend on `local.engine`. A closed match, so adding
/// a new [`coco_config::LocalSttEngine`] variant is a compile error until wired.
fn create_local_engine(
    config: &VoiceConfig,
    events: Option<mpsc::Sender<VoiceEvent>>,
) -> Result<Arc<dyn VoiceEngine>, VoiceError> {
    match config.local.engine {
        coco_config::LocalSttEngine::Whisper => {
            #[cfg(feature = "local-voice")]
            {
                let engine =
                    crate::local::LocalWhisperEngine::new(config.local.whisper.clone(), events);
                Ok(Arc::new(engine))
            }
            #[cfg(not(feature = "local-voice"))]
            {
                let _ = events;
                Err(VoiceError::FeatureNotEnabled("local"))
            }
        }
    }
}
