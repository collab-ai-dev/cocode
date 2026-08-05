//! The voice recording state machine — the one thing app/tui drives.
//!
//! `Idle -> Recording -> Transcribing -> Idle`. Recording start is synchronous
//! (spawns the capture thread and returns); stop kicks off an async task that
//! blocks-off-runtime to finalize the WAV, transcribes, and emits the result
//! over an opt-in event sink. `VoiceEvent` is an isolated stream (NOT bridged
//! into `CoreEvent`) — only the final inserted text ever touches user input.

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use coco_utils_audio::AudioCapture;
use coco_utils_audio::RecordingHandle;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::engine::TranscribeParams;
use crate::engine::VoiceCapabilities;
use crate::engine::VoiceEngine;
use crate::engine::VoiceProgress;
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
    RecordingStarted { generation: u64 },
    /// On-device model weights are downloading during first-use transcription.
    /// Carries cumulative bytes for a progress indicator; `total` is the size
    /// when the server reports it. Emitted only by the local backend.
    Download {
        generation: u64,
        model: String,
        received: u64,
        total: Option<u64>,
    },
    /// Recording stopped; transcription started (carries the backend name for
    /// the footer, e.g. "Transcribing via openai...").
    Transcribing { generation: u64, engine: String },
    /// Final transcript, ready to insert at the cursor.
    Final {
        generation: u64,
        text: String,
        language: Option<String>,
    },
    /// A user-facing failure; the session has returned to Idle.
    Error { generation: u64, message: String },
}

struct Active {
    recording: Box<dyn RecordingHandle>,
    cancel: CancellationToken,
    generation: u64,
}

/// Orchestrates capture + transcription for one session.
pub struct VoiceSession {
    engine: Arc<dyn VoiceEngine>,
    capture: Arc<dyn AudioCapture>,
    params: TranscribeParams,
    runtime: tokio::runtime::Handle,
    event_tx: Option<mpsc::Sender<VoiceEvent>>,
    active: Option<Active>,
    /// Cancel handle for the in-flight transcription (and its first-use model
    /// download). Set on `stop()` and fired by `cancel_transcription()` so a
    /// stuck download can be aborted — `active` is already `None` by then, so
    /// `cancel()` can't reach it.
    transcribe_cancel: Option<CancellationToken>,
    /// Monotonic operation generation shared with spawned transcription tasks.
    /// A cancelled/replaced task may still finish at the provider boundary, but
    /// it can never publish into a newer recording's UI state.
    generation: Arc<AtomicU64>,
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
            transcribe_cancel: None,
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Attach the event sink the TUI select-loop listens on.
    pub fn set_event_sink(&mut self, tx: mpsc::Sender<VoiceEvent>) {
        self.event_tx = Some(tx);
    }

    /// Update the dictation language (from `/voice-config lang`). `None` = auto.
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

    /// Current operation generation for consumers that need to invalidate
    /// already-queued events synchronously after cancel/replacement.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Start recording. Idempotent while already recording.
    pub fn start(&mut self) -> Result<(), VoiceError> {
        if self.active.is_some() {
            return Ok(());
        }
        if !self.capture.is_available() {
            return Err(VoiceError::NoAudioDevice);
        }
        self.cancel_transcription();
        let generation = next_generation(&self.generation);
        let recording = self.capture.start()?;
        self.active = Some(Active {
            recording,
            cancel: CancellationToken::new(),
            generation,
        });
        self.emit(VoiceEvent::RecordingStarted { generation });
        Ok(())
    }

    /// Stop recording and asynchronously transcribe. Emits `Final` or `Error`
    /// on the sink later. No-op if not recording.
    pub fn stop(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        self.emit(VoiceEvent::Transcribing {
            generation: active.generation,
            engine: self.engine.name().to_string(),
        });

        let engine = self.engine.clone();
        let params = self.params.clone();
        let event_tx = self.event_tx.clone();
        let cancel = active.cancel.clone();
        let generation = active.generation;
        let current_generation = Arc::clone(&self.generation);
        let recording = active.recording;
        // Retain the cancel handle so the transcription (and any first-use model
        // download) can still be aborted after `active` is cleared.
        self.transcribe_cancel = Some(active.cancel);

        self.runtime.spawn(async move {
            // Mark the retained operation token terminal on every exit path,
            // including capture failure. This lets the synchronous owner
            // distinguish a completed operation from one that still needs
            // cancellation without sharing mutable task state.
            let _completion = CancelOnDrop(cancel.clone());
            // Finalizing the WAV blocks (drains the capture thread) — keep it
            // off the async runtime.
            let audio = match tokio::task::spawn_blocking(move || recording.stop()).await {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(e)) => {
                    return send_error_if_current(
                        &event_tx,
                        VoiceError::from(e),
                        &current_generation,
                        generation,
                        &cancel,
                    )
                    .await;
                }
                Err(_) => {
                    return send_error_if_current(
                        &event_tx,
                        VoiceError::TranscriptionFailed("capture task panicked".to_string()),
                        &current_generation,
                        generation,
                        &cancel,
                    )
                    .await
                }
            };
            let (progress_tx, mut progress_rx) = mpsc::channel(32);
            let progress_events = event_tx.clone();
            let progress_generation = Arc::clone(&current_generation);
            let progress_cancel = cancel.clone();
            let progress_task = tokio::spawn(async move {
                while let Some(progress) = progress_rx.recv().await {
                    send_event_if_current(
                        &progress_events,
                        voice_event_from_progress(progress, generation),
                        &progress_generation,
                        generation,
                        &progress_cancel,
                    )
                    .await;
                }
            });
            let result = engine
                .transcribe(audio, &params, cancel.clone(), Some(progress_tx))
                .await;
            progress_task.abort();
            match result {
                Ok(transcript) => {
                    send_event_if_current(
                        &event_tx,
                        VoiceEvent::Final {
                            generation,
                            text: transcript.text,
                            language: transcript.language,
                        },
                        &current_generation,
                        generation,
                        &cancel,
                    )
                    .await;
                }
                Err(e) => {
                    send_error_if_current(&event_tx, e, &current_generation, generation, &cancel)
                        .await;
                }
            }
        });
    }

    /// Cancel an in-flight recording, discarding audio. Returns to Idle.
    pub fn cancel(&mut self) {
        if let Some(active) = self.active.take() {
            active.cancel.cancel();
            next_generation(&self.generation);
            // Dropping the recording handle stops the capture stream.
            drop(active.recording);
        }
    }

    /// Cancel an in-flight transcription — including a stuck first-use model
    /// download — after `stop()` has handed recording off to the async task.
    /// No-op if nothing is transcribing.
    pub fn cancel_transcription(&mut self) {
        if let Some(cancel) = self.transcribe_cancel.take() {
            if !cancel.is_cancelled() {
                cancel.cancel();
                next_generation(&self.generation);
            }
        }
    }

    fn emit(&self, event: VoiceEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(event);
        }
    }
}

struct CancelOnDrop(CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn next_generation(generation: &AtomicU64) -> u64 {
    generation.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
}

fn voice_event_from_progress(progress: VoiceProgress, generation: u64) -> VoiceEvent {
    match progress {
        VoiceProgress::Download {
            model,
            received,
            total,
        } => VoiceEvent::Download {
            generation,
            model,
            received,
            total,
        },
    }
}

async fn send_error_if_current(
    tx: &Option<mpsc::Sender<VoiceEvent>>,
    error: VoiceError,
    current_generation: &AtomicU64,
    generation: u64,
    cancel: &CancellationToken,
) {
    send_event_if_current(
        tx,
        VoiceEvent::Error {
            generation,
            message: error.to_string(),
        },
        current_generation,
        generation,
        cancel,
    )
    .await;
}

async fn send_event_if_current(
    tx: &Option<mpsc::Sender<VoiceEvent>>,
    event: VoiceEvent,
    current_generation: &AtomicU64,
    generation: u64,
    cancel: &CancellationToken,
) {
    if current_generation.load(Ordering::Acquire) != generation {
        return;
    }
    if let Some(tx) = tx {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {}
            _ = tx.send(event) => {}
        }
    }
}

#[cfg(test)]
#[path = "session.test.rs"]
mod tests;
