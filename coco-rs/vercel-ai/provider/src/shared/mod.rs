//! Shared types module.
//!
//! This module provides shared types organized by version.

pub mod v4;

// Re-export v4 types at this level for backward compatibility
pub use v4::FileRawData;
pub use v4::ProviderMetadata;
pub use v4::ProviderOptions;
pub use v4::SharedV4FileData;
pub use v4::Warning;
