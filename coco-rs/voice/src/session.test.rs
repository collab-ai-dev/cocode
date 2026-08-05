use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use async_trait::async_trait;
use coco_utils_audio::AudioCapture;
use coco_utils_audio::AudioCaptureError;
use coco_utils_audio::RecordingHandle;
use tokio::sync::mpsc;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::engine::Transcript;

struct TestCapture;

impl AudioCapture for TestCapture {
    fn is_available(&self) -> bool {
        true
    }

    fn start(&self) -> Result<Box<dyn RecordingHandle>, AudioCaptureError> {
        Ok(Box::new(TestRecording))
    }
}

struct TestRecording;

impl RecordingHandle for TestRecording {
    fn stop(self: Box<Self>) -> Result<Vec<u8>, AudioCaptureError> {
        Ok(vec![1, 2, 3])
    }
}

struct ControlledEngine {
    calls: AtomicUsize,
    started: mpsc::UnboundedSender<usize>,
    completed: mpsc::UnboundedSender<usize>,
    gates: [Notify; 2],
}

#[async_trait]
impl VoiceEngine for ControlledEngine {
    fn name(&self) -> &str {
        "controlled"
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
        _audio: Vec<u8>,
        _params: &TranscribeParams,
        _cancel: CancellationToken,
        progress: Option<mpsc::Sender<VoiceProgress>>,
    ) -> Result<Transcript, VoiceError> {
        let call = self.calls.fetch_add(1, Ordering::AcqRel);
        let _ = self.started.send(call);
        self.gates[call].notified().await;
        if call == 0 {
            if let Some(progress) = progress {
                let _ = progress
                    .send(VoiceProgress::Download {
                        model: "stale".to_string(),
                        received: 1,
                        total: Some(2),
                    })
                    .await;
                tokio::task::yield_now().await;
            }
        }
        let _ = self.completed.send(call);
        Ok(Transcript {
            text: format!("transcript-{call}"),
            language: None,
        })
    }
}

#[tokio::test]
async fn replaced_transcription_cannot_publish_stale_result() {
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let (completed_tx, mut completed_rx) = mpsc::unbounded_channel();
    let engine = Arc::new(ControlledEngine {
        calls: AtomicUsize::new(0),
        started: started_tx,
        completed: completed_tx,
        gates: [Notify::new(), Notify::new()],
    });
    let mut session = VoiceSession::new(
        engine.clone(),
        Arc::new(TestCapture),
        TranscribeParams::default(),
    );
    let (event_tx, mut event_rx) = mpsc::channel(16);
    session.set_event_sink(event_tx);

    session.start().expect("first recording");
    session.stop();
    assert_eq!(started_rx.recv().await, Some(0));

    session.start().expect("replacement recording");
    session.stop();
    assert_eq!(started_rx.recv().await, Some(1));

    // Discard synchronous lifecycle events; only terminal output matters here.
    while event_rx.try_recv().is_ok() {}
    engine.gates[0].notify_one();
    assert_eq!(completed_rx.recv().await, Some(0));
    tokio::task::yield_now().await;
    assert!(event_rx.try_recv().is_err());

    engine.gates[1].notify_one();
    assert_eq!(completed_rx.recv().await, Some(1));
    match tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
        .await
        .expect("current generation event")
        .expect("event channel")
    {
        VoiceEvent::Final { text, .. } => assert_eq!(text, "transcript-1"),
        other => panic!("expected current Final event, got {other:?}"),
    }
}
