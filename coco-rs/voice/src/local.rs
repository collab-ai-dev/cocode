//! On-device Whisper backend (feature = "local-voice").
//!
//! Mirrors coco/retrieval's local-model recipe: an optional heavy dependency
//! (whisper.cpp via `whisper-rs`) behind a cargo feature, a fallible
//! constructor (never `unwrap` on model init), the model kept warm in an `Arc`
//! for the session, and inference offloaded to `spawn_blocking` (a pass is
//! seconds — not the inline-sync shortcut retrieval uses for embeddings).
//!
//! Weights are NOT auto-downloaded yet: the model file must be present at the
//! resolved cache path. Download-on-first-use is a follow-up (needs an HTTP
//! client + HuggingFace URLs).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use whisper_rs::FullParams;
use whisper_rs::SamplingStrategy;
use whisper_rs::WhisperContext;
use whisper_rs::WhisperContextParameters;

use coco_config::LocalWhisperConfig;

use crate::engine::TranscribeParams;
use crate::engine::Transcript;
use crate::engine::VoiceCapabilities;
use crate::engine::VoiceEngine;
use crate::error::VoiceError;

/// On-device Whisper transcription. The loaded context is shared across
/// requests (loaded once, kept warm for the session).
pub struct LocalWhisperEngine {
    ctx: Arc<WhisperContext>,
}

impl LocalWhisperEngine {
    /// Load the Whisper model. Fails (never panics) when the weights are
    /// missing or unreadable — the factory surfaces the error to `/voice`.
    pub fn try_new(config: &LocalWhisperConfig) -> Result<Self, VoiceError> {
        let model_path = resolve_model_path(config);
        if !model_path.exists() {
            return Err(VoiceError::TranscriptionFailed(format!(
                "Whisper model not found at {}. Download a ggml model for `{}` \
                 (e.g. from https://huggingface.co/ggerganov/whisper.cpp) to that path.",
                model_path.display(),
                config.model
            )));
        }
        let ctx = WhisperContext::new_with_params(
            &model_path.to_string_lossy(),
            WhisperContextParameters::default(),
        )
        .map_err(|e| {
            VoiceError::TranscriptionFailed(format!("failed to load Whisper model: {e}"))
        })?;
        Ok(Self { ctx: Arc::new(ctx) })
    }
}

#[async_trait]
impl VoiceEngine for LocalWhisperEngine {
    fn name(&self) -> &str {
        "local"
    }

    fn capabilities(&self) -> VoiceCapabilities {
        VoiceCapabilities {
            requires_network: false,
            on_device: true,
            streaming: false,
        }
    }

    async fn transcribe(
        &self,
        audio: Vec<u8>,
        params: &TranscribeParams,
        cancel: CancellationToken,
    ) -> Result<Transcript, VoiceError> {
        if cancel.is_cancelled() {
            return Err(VoiceError::Cancelled);
        }
        let ctx = self.ctx.clone();
        let language = params.language.clone();
        // A Whisper pass is seconds of CPU — keep it off the async runtime.
        let text =
            tokio::task::spawn_blocking(move || run_whisper(&ctx, &audio, language.as_deref()))
                .await
                .map_err(|e| {
                    VoiceError::TranscriptionFailed(format!("whisper task panicked: {e}"))
                })??;

        let text = text.trim().to_string();
        if text.is_empty() {
            return Err(VoiceError::NoSpeechDetected);
        }
        Ok(Transcript {
            text,
            language: params.language.clone(),
        })
    }
}

fn run_whisper(
    ctx: &WhisperContext,
    wav: &[u8],
    language: Option<&str>,
) -> Result<String, VoiceError> {
    let samples = decode_wav_to_f32(wav)?;
    let mut state = ctx
        .create_state()
        .map_err(|e| VoiceError::TranscriptionFailed(format!("whisper state: {e}")))?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    if let Some(lang) = language {
        params.set_language(Some(lang));
    }
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    state
        .full(params, &samples)
        .map_err(|e| VoiceError::TranscriptionFailed(format!("whisper inference: {e}")))?;

    let segments = state
        .full_n_segments()
        .map_err(|e| VoiceError::TranscriptionFailed(format!("whisper segments: {e}")))?;
    let mut out = String::new();
    for i in 0..segments {
        let segment = state
            .full_get_segment_text(i)
            .map_err(|e| VoiceError::TranscriptionFailed(format!("whisper segment {i}: {e}")))?;
        out.push_str(&segment);
    }
    Ok(out)
}

/// Decode 16 kHz mono 16-bit PCM WAV bytes (what capture produces) to `f32`
/// samples in `[-1, 1]`, as Whisper expects.
fn decode_wav_to_f32(wav: &[u8]) -> Result<Vec<f32>, VoiceError> {
    let reader = hound::WavReader::new(std::io::Cursor::new(wav))
        .map_err(|e| VoiceError::TranscriptionFailed(format!("decode WAV: {e}")))?;
    let samples = reader
        .into_samples::<i16>()
        .map(|s| s.map(|v| f32::from(v) / f32::from(i16::MAX)))
        .collect::<Result<Vec<f32>, _>>()
        .map_err(|e| VoiceError::TranscriptionFailed(format!("read WAV samples: {e}")))?;
    Ok(samples)
}

/// Resolve the on-disk model path: `<cache_dir>/ggml-<model>.bin`, defaulting
/// the cache dir to `<config_home>/models/whisper/`.
fn resolve_model_path(config: &LocalWhisperConfig) -> PathBuf {
    let dir = config.cache_dir.clone().unwrap_or_else(|| {
        coco_config::global_config::config_home()
            .join("models")
            .join("whisper")
    });
    dir.join(format!("ggml-{}.bin", config.model))
}
