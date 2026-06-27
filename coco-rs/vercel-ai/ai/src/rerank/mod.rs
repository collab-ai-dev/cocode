//! Rerank module for document reranking.
//!
//! This module provides `rerank` function for reranking documents
//! using a reranking model.

#[allow(clippy::module_inception)]
mod rerank;
mod rerank_result;

pub use rerank::RerankOptions;
pub use rerank::RerankingModel;
pub use rerank::rerank;
pub use rerank_result::RerankResponse;
pub use rerank_result::RerankResult;
pub use rerank_result::RerankedDocument;
