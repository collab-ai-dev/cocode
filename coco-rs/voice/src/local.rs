//! On-device Whisper backend (feature = "local-voice").
//!
//! Mirrors coco/retrieval's local-model recipe: an optional heavy dependency
//! (whisper.cpp via `whisper-rs`) behind a cargo feature, model init that never
//! `unwrap`s, the context kept warm in an `Arc` for the session, and inference
//! offloaded to `spawn_blocking` (a pass is seconds — not the inline-sync
//! shortcut retrieval uses for embeddings).
//!
//! The model is loaded lazily on the first `transcribe`, not at construction,
//! so enabling `voice.backend = local` never blocks or bloats startup when the
//! mic is never used. [`LocalWhisperEngine::ensure_ctx`] is the single load
//! choke point — and the site where download-on-first-use hooks in.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::sync::OnceCell;
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
use crate::session::VoiceEvent;

/// On-device Whisper transcription. Construction is cheap and infallible; the
/// context is loaded once on first use and then shared (kept warm) across
/// requests for the session.
pub struct LocalWhisperEngine {
    config: LocalWhisperConfig,
    ctx: OnceCell<Arc<WhisperContext>>,
    /// Voice event stream, used to report weight-download progress on first use.
    events: Option<mpsc::Sender<VoiceEvent>>,
}

impl LocalWhisperEngine {
    /// Build the engine without touching disk. The model loads (and, if
    /// missing, downloads) on the first `transcribe` call via
    /// [`Self::ensure_ctx`]. `events` receives download progress.
    pub fn new(config: LocalWhisperConfig, events: Option<mpsc::Sender<VoiceEvent>>) -> Self {
        Self {
            config,
            ctx: OnceCell::new(),
            events,
        }
    }

    /// Load-once accessor for the Whisper context. On first use it downloads the
    /// weights if they are missing and eligible for auto-download (a known,
    /// checksum-pinned model), then loads them. Fails (never panics) when the
    /// weights are missing and cannot be auto-fetched, or are unreadable. The
    /// heavy `WhisperContext::new_with_params` load is kept off the async
    /// runtime via `spawn_blocking`.
    async fn ensure_ctx(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Arc<WhisperContext>, VoiceError> {
        let ctx = self
            .ctx
            .get_or_try_init(|| async {
                let model_path = crate::models::resolve_model_path(&self.config);
                if !model_path.exists() {
                    if !crate::models::may_auto_download(&self.config) {
                        return Err(VoiceError::TranscriptionFailed(format!(
                            "Whisper model `{}` is missing at {} and won't auto-download \
                             (auto-download applies only to a built-in model with \
                             `auto_download` on and no custom `model_url`). Pick a built-in \
                             model with `/voice-config local model <name>` (e.g. base.en, \
                             small), or place the ggml weights at that path yourself.",
                            self.config.model,
                            model_path.display()
                        )));
                    }
                    crate::download::download_model(
                        &self.config,
                        self.events.clone(),
                        cancel.clone(),
                    )
                    .await?;
                }
                let path = model_path.to_string_lossy().to_string();
                tokio::task::spawn_blocking(move || {
                    WhisperContext::new_with_params(&path, WhisperContextParameters::default())
                        .map(Arc::new)
                        .map_err(|e| {
                            VoiceError::TranscriptionFailed(format!(
                                "failed to load Whisper model: {e}"
                            ))
                        })
                })
                .await
                .map_err(|e| {
                    VoiceError::TranscriptionFailed(format!("whisper load task panicked: {e}"))
                })?
            })
            .await?;
        Ok(ctx.clone())
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
        let ctx = self.ensure_ctx(&cancel).await?;
        if cancel.is_cancelled() {
            return Err(VoiceError::Cancelled);
        }
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
    // Whisper defaults to a conservative 4 threads; use available parallelism
    // (capped) so multi-core machines aren't left idle during the CPU pass.
    let threads = std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(4)
        .clamp(1, 8) as i32;
    params.set_n_threads(threads);
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
