//! Embedding model module.
//!
//! This module provides embedding model types organized by version.

pub mod v4;

// Re-export v4 types at this level for backward compatibility
pub use v4::EmbeddingModelV4;
pub use v4::EmbeddingModelV4CallOptions;
pub use v4::EmbeddingModelV4EmbedResult;
pub use v4::EmbeddingType;
pub use v4::EmbeddingUsage;
pub use v4::EmbeddingValue;
