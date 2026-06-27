//! Token counting utilities.
//!
//! Provides token counting for statistics and debugging.
//! Uses tiktoken (cl100k_base) for OpenAI-compatible token counting.
//!
//! Note: Chunk splitting is handled natively by CodeSplitter in token mode.
//! This module is only for statistics/validation purposes.

use tiktoken_rs::cl100k_base;

/// Default maximum tokens per chunk for embedding models.
pub const DEFAULT_MAX_CHUNK_TOKENS: usize = 512;

/// Token counter for statistics and validation.
pub struct TokenCounter {
    bpe: tiktoken_rs::CoreBPE,
    max_tokens: usize,
}

impl TokenCounter {
    /// Create a new token counter with default max tokens (512).
    pub fn new() -> Self {
        Self::with_max_tokens(DEFAULT_MAX_CHUNK_TOKENS)
    }

    /// Create a new token counter with custom max tokens.
    pub fn with_max_tokens(max_tokens: usize) -> Self {
        let bpe = cl100k_base().expect("Failed to load cl100k_base tokenizer");
        Self { bpe, max_tokens }
    }

    /// Count tokens in a text string.
    pub fn count_tokens(&self, text: &str) -> usize {
        self.bpe.encode_with_special_tokens(text).len()
    }

    /// Check if a chunk is within the token limit.
    pub fn is_valid(&self, chunk: &str) -> bool {
        self.count_tokens(chunk) <= self.max_tokens
    }

    /// Get the maximum tokens limit.
    pub fn max_tokens(&self) -> usize {
        self.max_tokens
    }
}

impl Default for TokenCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "validation.test.rs"]
mod tests;
