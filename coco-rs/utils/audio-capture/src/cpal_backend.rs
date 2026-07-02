//! cpal microphone backend (feature = "cpal").
//!
//! cpal's `Stream` is `!Send` on several platforms, so every cpal object lives
//! on one dedicated OS thread; the async world talks to it purely over
//! `std::sync::mpsc` channels.

use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;

use cpal::traits::DeviceTrait;
use cpal::traits::HostTrait;
use cpal::traits::StreamTrait;
use cpal::SampleFormat;

use crate::capture::AudioCapture;
use crate::capture::RecordingHandle;
use crate::error::AudioCaptureError;
use crate::wav::encode_wav_16k_mono;

type SharedBuffer = Arc<Mutex<Vec<f32>>>;

/// cpal-backed microphone capture.
pub struct CpalCapture;

impl CpalCapture {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CpalCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCapture for CpalCapture {
    fn is_available(&self) -> bool {
        cpal::default_host().default_input_device().is_some()
    }

    fn start(&self) -> Result<Box<dyn RecordingHandle>, AudioCaptureError> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (result_tx, result_rx) = mpsc::channel::<Result<Vec<u8>, AudioCaptureError>>();
        // Report stream construction success/failure before `start` returns, so
        // a missing device or denied permission surfaces synchronously.
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), AudioCaptureError>>();

        std::thread::Builder::new()
            .name("coco-audio-capture".to_string())
            .spawn(move || run_capture_thread(&stop_rx, &result_tx, &ready_tx))
            .map_err(|e| AudioCaptureError::Backend(e.to_string()))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Box::new(CpalRecording { stop_tx, result_rx })),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(AudioCaptureError::Backend(
                "capture thread exited during startup".to_string(),
            )),
        }
    }
}

struct CpalRecording {
    stop_tx: mpsc::Sender<()>,
    result_rx: mpsc::Receiver<Result<Vec<u8>, AudioCaptureError>>,
}

impl RecordingHandle for CpalRecording {
    fn stop(self: Box<Self>) -> Result<Vec<u8>, AudioCaptureError> {
        // If the thread already exited, the send fails and `recv` surfaces it.
        let _ = self.stop_tx.send(());
        match self.result_rx.recv() {
            Ok(result) => result,
            Err(_) => Err(AudioCaptureError::Backend(
                "capture thread ended before returning audio".to_string(),
            )),
        }
    }
}

fn run_capture_thread(
    stop_rx: &mpsc::Receiver<()>,
    result_tx: &mpsc::Sender<Result<Vec<u8>, AudioCaptureError>>,
    ready_tx: &mpsc::Sender<Result<(), AudioCaptureError>>,
) {
    let (stream, buffer, sample_rate) = match build_stream() {
        Ok(built) => built,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };
    if let Err(e) = stream
        .play()
        .map_err(|e| AudioCaptureError::Backend(e.to_string()))
    {
        let _ = ready_tx.send(Err(e));
        return;
    }
    let _ = ready_tx.send(Ok(()));

    // Block until asked to stop (or the handle is dropped).
    let _ = stop_rx.recv();

    // Stopping the stream: dropping it closes the device.
    let _ = stream.pause();
    drop(stream);

    let samples = {
        let guard = buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.clone()
    };
    if samples.is_empty() {
        let _ = result_tx.send(Err(AudioCaptureError::NoAudioCaptured));
        return;
    }
    let _ = result_tx.send(encode_wav_16k_mono(&samples, sample_rate));
}

fn build_stream() -> Result<(cpal::Stream, SharedBuffer, u32), AudioCaptureError> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or(AudioCaptureError::NoInputDevice)?;
    let supported = device
        .default_input_config()
        .map_err(|e| AudioCaptureError::Backend(e.to_string()))?;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels as usize;
    let sample_rate = config.sample_rate.0;

    let buffer: SharedBuffer = Arc::new(Mutex::new(Vec::new()));
    let err_fn = |e: cpal::StreamError| tracing::warn!(error = %e, "audio input stream error");

    let stream = match sample_format {
        SampleFormat::F32 => {
            let buf = buffer.clone();
            device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    push_mono(&buf, data, channels, |s| s)
                },
                err_fn,
                None,
            )
        }
        SampleFormat::I16 => {
            let buf = buffer.clone();
            device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    push_mono(&buf, data, channels, |s| f32::from(s) / f32::from(i16::MAX))
                },
                err_fn,
                None,
            )
        }
        SampleFormat::U16 => {
            let buf = buffer.clone();
            device.build_input_stream(
                &config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    push_mono(&buf, data, channels, |s| (f32::from(s) - 32768.0) / 32768.0)
                },
                err_fn,
                None,
            )
        }
        other => return Err(AudioCaptureError::UnsupportedFormat(format!("{other:?}"))),
    }
    .map_err(|e| AudioCaptureError::Backend(e.to_string()))?;

    Ok((stream, buffer, sample_rate))
}

/// Downmix interleaved multi-channel samples to mono `f32` and append.
fn push_mono<T: Copy>(
    buffer: &SharedBuffer,
    data: &[T],
    channels: usize,
    to_f32: impl Fn(T) -> f32,
) {
    if channels == 0 {
        return;
    }
    let mut guard = buffer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for frame in data.chunks(channels) {
        let sum: f32 = frame.iter().map(|&s| to_f32(s)).sum();
        guard.push(sum / channels as f32);
    }
}
