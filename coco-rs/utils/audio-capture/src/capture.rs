//! The capture seam: a backend-agnostic trait plus the default factory.
//!
//! No cpal types leak through this interface (mirrors `utils/pty` wrapping
//! `portable-pty`) — callers depend only on `AudioCapture` / `RecordingHandle`.

use std::sync::Arc;

use crate::error::AudioCaptureError;

/// Probe for and start microphone capture.
pub trait AudioCapture: Send + Sync {
    /// Whether a usable input device exists. Cheap — enumerates the default
    /// device but does NOT open a stream, so it never triggers the macOS
    /// microphone-permission (TCC) dialog. Used by the `/voice` enable
    /// pre-flight before any recording is attempted.
    fn is_available(&self) -> bool;

    /// Begin capturing from the default input device. The returned handle keeps
    /// the stream alive until `stop` is called.
    fn start(&self) -> Result<Box<dyn RecordingHandle>, AudioCaptureError>;
}

/// A live recording. [`stop`](RecordingHandle::stop) **blocks** until the
/// stream is drained and the samples are resampled + encoded to 16 kHz mono
/// WAV bytes — call it off the async runtime (e.g. `tokio::task::spawn_blocking`).
pub trait RecordingHandle: Send {
    fn stop(self: Box<Self>) -> Result<Vec<u8>, AudioCaptureError>;
}

/// Default capture backend for this build: the cpal microphone backend when the
/// `cpal` feature is compiled, otherwise an unsupported stub whose
/// `is_available()` returns `false`.
pub fn default_capture() -> Arc<dyn AudioCapture> {
    #[cfg(feature = "cpal")]
    {
        Arc::new(crate::cpal_backend::CpalCapture::new())
    }
    #[cfg(not(feature = "cpal"))]
    {
        Arc::new(UnsupportedCapture)
    }
}

/// Stub used when microphone capture is not compiled in (`cpal` feature off).
#[cfg(not(feature = "cpal"))]
struct UnsupportedCapture;

#[cfg(not(feature = "cpal"))]
impl AudioCapture for UnsupportedCapture {
    fn is_available(&self) -> bool {
        false
    }

    fn start(&self) -> Result<Box<dyn RecordingHandle>, AudioCaptureError> {
        Err(AudioCaptureError::NotCompiled)
    }
}
