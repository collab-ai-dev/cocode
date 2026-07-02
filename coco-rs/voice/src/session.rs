//! The voice recording state machine — the one thing app/tui drives.
//!
//! `Idle -> Recording -> Transcribing -> Idle`. Recording start is synchronous
//! (spawns the capture thread and returns); stop kicks off an async task that
//! blocks-off-runtime to finalize the WAV, transcribes, and emits the result
//! over an opt-in event sink. `VoiceEvent` is an isolated stream (NOT bridged
//! into `CoreEvent`) — only the final inserted text ever touches user input.

use std::sync::Arc;

use coco_utils_audio::AudioCapture;
use coco_utils_audio::RecordingHandle;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::engine::TranscribeParams;
use crate::engine::VoiceCapabilities;
use crate::engine::VoiceEngine;
use crate::error::VoiceError;

/// Display state of a voice session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoiceState {
    /// Not recording.
    #[default]
    Idle,
    /// Microphone is live.
    Recording,
    /// Recording stopped; transcription in flight.
    Transcribing,
}

/// Lifecycle events emitted by a session. Isolated stream — the app folds these
/// into its own UI state and inserts only the final text.
#[derive(Debug, Clone)]
pub enum VoiceEvent {
    /// The microphone is now capturing.
    RecordingStarted,
    /// Recording stopped; transcription started (carries the backend name for
    /// the footer, e.g. "Transcribing via openai...").
    Transcribing { engine: String },
    /// Final transcript, ready to insert at the cursor.
    Final {
        text: String,
        language: Option<String>,
    },
    /// A user-facing failure; the session has returned to Idle.
    Error(String),
}

struct Active {
    recording: Box<dyn RecordingHandle>,
    cancel: CancellationToken,
}

/// Orchestrates capture + transcription for one session.
pub struct VoiceSession {
    engine: Arc<dyn VoiceEngine>,
    capture: Arc<dyn AudioCapture>,
    params: TranscribeParams,
    runtime: tokio::runtime::Handle,
    event_tx: Option<mpsc::Sender<VoiceEvent>>,
    active: Option<Active>,
}

impl VoiceSession {
    /// Construct a session. Must be called from within a Tokio runtime (the
    /// current handle is captured for spawning the transcription task).
    pub fn new(
        engine: Arc<dyn VoiceEngine>,
        capture: Arc<dyn AudioCapture>,
        params: TranscribeParams,
    ) -> Self {
        Self {
            engine,
            capture,
            params,
            runtime: tokio::runtime::Handle::current(),
            event_tx: None,
            active: None,
        }
    }

    /// Attach the event sink the TUI select-loop listens on.
    pub fn set_event_sink(&mut self, tx: mpsc::Sender<VoiceEvent>) {
        self.event_tx = Some(tx);
    }

    /// Update the dictation language (from `/voice-lang`). `None` = auto-detect.
    pub fn set_language(&mut self, language: Option<String>) {
        self.params.language = language;
    }

    /// Whether a usable input device exists (no stream is opened).
    pub fn is_available(&self) -> bool {
        self.capture.is_available()
    }

    /// Backend name for status text.
    pub fn engine_name(&self) -> &str {
        self.engine.name()
    }

    /// Backend capabilities (privacy posture, streaming).
    pub fn capabilities(&self) -> VoiceCapabilities {
        self.engine.capabilities()
    }

    /// Whether the microphone is currently recording.
    pub fn is_recording(&self) -> bool {
        self.active.is_some()
    }

    /// Start recording. Idempotent while already recording.
    pub fn start(&mut self) -> Result<(), VoiceError> {
        if self.active.is_some() {
            return Ok(());
        }
        if !self.capture.is_available() {
            return Err(VoiceError::NoAudioDevice);
        }
        let recording = self.capture.start()?;
        self.active = Some(Active {
            recording,
            cancel: CancellationToken::new(),
        });
        self.emit(VoiceEvent::RecordingStarted);
        Ok(())
    }

    /// Stop recording and asynchronously transcribe. Emits `Final` or `Error`
    /// on the sink later. No-op if not recording.
    pub fn stop(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        self.emit(VoiceEvent::Transcribing {
            engine: self.engine.name().to_string(),
        });

        let engine = self.engine.clone();
        let params = self.params.clone();
        let event_tx = self.event_tx.clone();
        let cancel = active.cancel.clone();
        let recording = active.recording;

        self.runtime.spawn(async move {
            // Finalizing the WAV blocks (drains the capture thread) — keep it
            // off the async runtime.
            let audio = match tokio::task::spawn_blocking(move || recording.stop()).await {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(e)) => return send_error(&event_tx, VoiceError::from(e)).await,
                Err(_) => {
                    return send_error(
                        &event_tx,
                        VoiceError::TranscriptionFailed("capture task panicked".to_string()),
                    )
                    .await
                }
            };
            match engine.transcribe(audio, &params, cancel).await {
                Ok(transcript) => {
                    if let Some(tx) = &event_tx {
                        let _ = tx
                            .send(VoiceEvent::Final {
                                text: transcript.text,
                                language: transcript.language,
                            })
                            .await;
                    }
                }
                Err(e) => send_error(&event_tx, e).await,
            }
        });
    }

    /// Cancel an in-flight recording, discarding audio. Returns to Idle.
    pub fn cancel(&mut self) {
        if let Some(active) = self.active.take() {
            active.cancel.cancel();
            // Dropping the recording handle stops the capture stream.
            drop(active.recording);
        }
    }

    fn emit(&self, event: VoiceEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(event);
        }
    }
}

async fn send_error(tx: &Option<mpsc::Sender<VoiceEvent>>, error: VoiceError) {
    if let Some(tx) = tx {
        let _ = tx.send(VoiceEvent::Error(error.to_string())).await;
    }
}
