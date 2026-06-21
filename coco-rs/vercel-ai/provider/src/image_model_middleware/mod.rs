//! Image model middleware module.
//!
//! This module provides middleware patterns for image models.

pub mod v4;

// Re-export v4 types at this level
pub use v4::ImageModelV4Middleware;
