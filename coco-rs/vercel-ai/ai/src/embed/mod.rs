//! Embedding generation module.
//!
//! This module provides `embed` and `embed_many` functions for generating
//! embeddings from text using embedding models.

mod embed_result;
mod generate;

pub use embed_result::EmbedManyResult;
pub use embed_result::EmbedResult;
pub use generate::EmbedManyOptions;
pub use generate::EmbedOptions;
pub use generate::embed;
pub use generate::embed_many;
