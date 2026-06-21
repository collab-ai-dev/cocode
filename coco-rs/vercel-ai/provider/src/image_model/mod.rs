//! Image model module.
//!
//! This module provides image model types organized by version.

pub mod v4;

// Re-export v4 types at this level for backward compatibility
pub use v4::GeneratedImage;
pub use v4::ImageData;
pub use v4::ImageFileData;
pub use v4::ImageModelV4;
pub use v4::ImageModelV4CallOptions;
pub use v4::ImageModelV4File;
pub use v4::ImageModelV4GenerateResult;
pub use v4::ImageQuality;
pub use v4::ImageResponseFormat;
pub use v4::ImageSize;
pub use v4::ImageStyle;
