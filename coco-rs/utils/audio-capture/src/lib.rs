//! Microphone capture for coco's voice input, normalized to the single
//! canonical STT format (16 kHz mono 16-bit PCM WAV).
//!
//! The public API ([`AudioCapture`] / [`RecordingHandle`]) is backend-agnostic;
//! the real cpal microphone backend is gated behind the `cpal` cargo feature so
//! the default workspace build pulls no platform audio system-deps. Without the
//! feature, [`default_capture`] returns a stub whose `is_available()` is `false`.

mod capture;
mod error;
mod wav;

#[cfg(feature = "cpal")]
mod cpal_backend;

pub use capture::default_capture;
pub use capture::AudioCapture;
pub use capture::RecordingHandle;
pub use error::AudioCaptureError;
pub use wav::encode_wav_16k_mono;
pub use wav::resample_to_16k;
pub use wav::TARGET_SAMPLE_RATE;
