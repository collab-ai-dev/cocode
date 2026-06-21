//! Video model module.
//!
//! This module provides video generation model types organized by version.

pub mod v4;

// Re-export v4 types at this level
pub use v4::GeneratedVideo;
pub use v4::VideoDuration;
pub use v4::VideoModelV4;
pub use v4::VideoModelV4CallOptions;
pub use v4::VideoModelV4Result;
pub use v4::VideoSize;
