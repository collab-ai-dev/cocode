//! Whisper weights download orchestration.
//!
//! Ungated (works without `local-voice`, e.g. to pre-stage weights) and shared
//! by both the first-use auto-download (`local::ensure_ctx`) and the explicit
//! `/voice-config download`. Bridges the downloader's lossy `DownloadProgress`
//! onto the voice event stream as [`VoiceEvent::Download`].

use std::path::PathBuf;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use coco_config::LocalWhisperConfig;
use coco_utils_download::DownloadProgress;

use crate::error::VoiceError;
use crate::models;
use crate::session::VoiceEvent;

/// `User-Agent` for weight downloads.
fn user_agent() -> String {
    format!("coco-voice/{}", env!("CARGO_PKG_VERSION"))
}

/// Download `config`'s whisper weights, verifying the pinned checksum for known
/// models, and forward progress to `events` as [`VoiceEvent::Download`].
/// Returns the installed path. `cancel` aborts mid-transfer (leaving no partial
/// file behind).
pub async fn download_model(
    config: &LocalWhisperConfig,
    events: Option<mpsc::Sender<VoiceEvent>>,
    cancel: CancellationToken,
) -> Result<PathBuf, VoiceError> {
    // Honor the user's opt-out of the progress indicator.
    let events = if config.show_download_progress {
        events
    } else {
        None
    };
    let req = models::build_download_request(config, user_agent());
    let dest = req.dest.clone();
    let model = config.model.clone();

    // Bridge lossy DownloadProgress → VoiceEvent::Download on the voice stream.
    let (dl_tx, mut dl_rx) = mpsc::channel::<DownloadProgress>(32);
    let bridge = tokio::spawn(async move {
        while let Some(progress) = dl_rx.recv().await {
            if let (Some(sink), Some(event)) = (&events, map_progress(&model, progress)) {
                let _ = sink.send(event).await;
            }
        }
    });

    let result = coco_utils_download::download_file(req, Some(dl_tx), cancel).await;
    // `dl_tx` was moved into `download_file`; its drop ends the bridge loop.
    let _ = bridge.await;

    match result {
        Ok(()) => Ok(dest),
        Err(coco_utils_download::DownloadError::Cancelled) => Err(VoiceError::Cancelled),
        Err(other) => Err(VoiceError::TranscriptionFailed(format!(
            "download whisper model `{}`: {other}",
            config.model
        ))),
    }
}

fn map_progress(model: &str, progress: DownloadProgress) -> Option<VoiceEvent> {
    let (received, total) = match progress {
        DownloadProgress::Started { total } => (0, total),
        DownloadProgress::Progress { received, total } => (received, total),
        DownloadProgress::Done { total } => (total, Some(total)),
        // The verify step has no meaningful byte count for the indicator.
        DownloadProgress::Verifying => return None,
    };
    Some(VoiceEvent::Download {
        model: model.to_string(),
        received,
        total,
    })
}
