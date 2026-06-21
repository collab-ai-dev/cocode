//! Embedding model middleware module.
//!
//! This module provides middleware patterns for embedding models.

pub mod v4;

// Re-export v4 types at this level
pub use v4::EmbeddingModelV4Middleware;
