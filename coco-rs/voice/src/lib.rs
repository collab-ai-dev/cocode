//! Voice input (speech-to-text dictation) for coco.
//!
//! One backend-agnostic [`VoiceEngine`] seam with a remote OpenAI-wire backend
//! (MVP) and an optional on-device Whisper backend (`local-voice` feature).
//! [`VoiceSession`] is the state machine app/tui drives; [`create_voice_engine`]
//! selects the backend the retrieval-`create_reranker` way.
//!
//! On/off is owned by `coco_types::Feature::Voice`; backend/language/model live
//! in `coco_config::VoiceConfig`.

mod download;
mod engine;
mod error;
mod factory;
mod models;
mod remote;
mod session;

#[cfg(feature = "local-voice")]
mod local;

pub use engine::TranscribeParams;
pub use engine::Transcript;
pub use engine::VoiceCapabilities;
pub use engine::VoiceEngine;
pub use error::VoiceError;
pub use factory::create_voice_engine;
pub use models::find_model;
pub use models::may_auto_download;
pub use models::resolve_model_path;
pub use models::WhisperModelSpec;
pub use models::KNOWN_MODELS;
pub use remote::RemoteOpenAiEngine;
pub use session::VoiceEvent;
pub use session::VoiceSession;
pub use session::VoiceState;

use coco_config::LocalWhisperConfig;
use coco_config::VoiceConfig;

/// Download the configured whisper weights (the explicit `/voice-config
/// download` path — the app runner calls this). Verifies the pinned checksum
/// for a known model and forwards progress to `events` as
/// [`VoiceEvent::Download`]. Available without the `local-voice` feature so
/// weights can be pre-staged. Returns the installed path.
pub async fn download_whisper_model(
    config: &LocalWhisperConfig,
    events: Option<tokio::sync::mpsc::Sender<VoiceEvent>>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<std::path::PathBuf, VoiceError> {
    download::download_model(config, events, cancel).await
}

/// Build [`TranscribeParams`] from resolved config (maps `"auto"` → `None`).
pub fn params_from_config(config: &VoiceConfig) -> TranscribeParams {
    let language = if config.language.is_empty() || config.language.eq_ignore_ascii_case("auto") {
        None
    } else {
        Some(config.language.clone())
    };
    TranscribeParams { language }
}
