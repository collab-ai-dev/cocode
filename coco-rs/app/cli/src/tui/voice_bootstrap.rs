//! Voice-input bootstrap: build the STT engine + capture + `VoiceSession` and
//! install it onto the TUI app.
//!
//! Gated at runtime by `Feature::Voice` — the session is only constructed when
//! voice is enabled at launch. Best-effort: any failure (no OpenAI credentials,
//! backend unavailable) logs and leaves the app without voice rather than
//! aborting startup. The real microphone backend requires the `voice` cargo
//! feature (`coco-voice/capture`); without it `default_capture()` returns a
//! stub whose `is_available()` is `false`, so recording surfaces "no mic".

use std::sync::Arc;

use coco_config::RuntimeConfig;
use coco_config::VoiceBackend;
use coco_inference::TranscriptionModelV4;
use coco_tui::App;
use coco_types::Feature;

use coco_agent_host::session_runtime::SessionHandle;

/// Install voice input onto `app` when `Feature::Voice` is enabled. Returns the
/// app unchanged (with a warning log) on any failure.
pub fn install_for_session(app: App, session: &SessionHandle) -> App {
    let runtime_config = session.runtime_config();
    if !runtime_config.features.enabled(Feature::Voice) {
        return app;
    }
    match build_voice_session(runtime_config) {
        Ok((session, rx)) => app.with_voice(session, rx, true),
        Err(err) => {
            tracing::warn!(error = %err, "voice input unavailable; leaving voice off");
            app
        }
    }
}

fn build_voice_session(
    runtime_config: &RuntimeConfig,
) -> anyhow::Result<(
    coco_voice::VoiceSession,
    tokio::sync::mpsc::Receiver<coco_voice::VoiceEvent>,
)> {
    let voice = &runtime_config.voice;
    let remote_handle: Option<Arc<dyn TranscriptionModelV4>> = match voice.backend {
        VoiceBackend::Remote => Some(build_remote_transcription(runtime_config)?),
        VoiceBackend::Local => None,
    };
    // One event stream feeds both recording lifecycle and download progress.
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let engine = coco_voice::create_voice_engine(voice, remote_handle, Some(tx.clone()))
        .map_err(|e| anyhow::anyhow!("voice engine: {e}"))?;
    let capture = coco_utils_audio::default_capture();
    let params = coco_voice::params_from_config(voice);
    let mut session = coco_voice::VoiceSession::new(engine, capture, params);
    session.set_event_sink(tx);
    Ok((session, rx))
}

/// Build an OpenAI-wire transcription model handle from the provider named by
/// `voice.remote.provider`, reusing that provider's resolved config +
/// credential resolver (so auth stays in the provider layer — this crate
/// injects a pre-built handle into `coco-voice`). Any OpenAI-wire host (OpenAI,
/// Groq, a self-hosted faster-whisper) works by pointing at its providers.json
/// entry.
fn build_remote_transcription(
    runtime_config: &RuntimeConfig,
) -> anyhow::Result<Arc<dyn TranscriptionModelV4>> {
    let remote = &runtime_config.voice.remote;
    let provider_cfg = runtime_config
        .providers
        .get(&remote.provider)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no `{}` provider configured for voice transcription; \
             set voice.remote.provider to a configured OpenAI-wire provider",
                remote.provider
            )
        })?;
    let resolver = coco_agent_host::provider_login::shared_resolver();
    coco_inference::build_openai_transcription_model(
        provider_cfg,
        Some(&resolver),
        &remote.model,
        /*timeout_secs*/ 60,
    )
    .map_err(|e| anyhow::anyhow!("build transcription model for `{}`: {e}", remote.provider))
}
