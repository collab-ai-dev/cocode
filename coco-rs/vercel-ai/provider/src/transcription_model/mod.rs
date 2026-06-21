//! Transcription model module.
//!
//! This module provides transcription (speech-to-text) model types organized by version.

pub mod v4;

// Re-export v4 types at this level
pub use v4::TranscriptionModelV4;
pub use v4::TranscriptionModelV4CallOptions;
pub use v4::TranscriptionModelV4Request;
pub use v4::TranscriptionModelV4Response;
pub use v4::TranscriptionModelV4Result;
pub use v4::TranscriptionSegmentV4;
