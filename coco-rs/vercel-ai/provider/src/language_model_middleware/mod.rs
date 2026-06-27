//! Language model middleware module.
//!
//! This module provides middleware patterns for language models.

pub mod v4;

// Re-export v4 types at this level
pub use v4::CallType;
pub use v4::LanguageModelV4Middleware;
pub use v4::MiddlewareOptions;
pub use v4::TransformParamsOptions;
pub use v4::WrapGenerateOptions;
pub use v4::WrapStreamOptions;
