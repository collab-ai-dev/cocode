//! Provider module.
//!
//! This module provides provider types organized by version.

pub mod v4;

// Re-export v4 types at this level for backward compatibility
pub use v4::ConfigurableProvider;
pub use v4::FromEnvProvider;
pub use v4::ProviderV4;
pub use v4::SimpleProvider;
