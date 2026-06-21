//! Token budgeting for repo map generation.
//!
//! Uses sampling-based estimation with narrowed binary search
//! to efficiently find the optimal number of symbols within token budget.

use std::sync::Arc;

use once_cell::sync::Lazy;
use tiktoken_rs::CoreBPE;

use crate::error::Result;
use crate::error::RetrievalErr;

use super::RankedSymbol;
use super::renderer::TreeRenderer;

/// Global shared TokenBudgeter instance.
/// Initialized lazily on first access, panics if tokenizer fails to load.
static GLOBAL_BUDGETER: Lazy<Arc<TokenBudgeter>> =
    Lazy::new(|| {
        Arc::new(TokenBudgeter::new_internal().expect(
            "Failed to initialize global TokenBudgeter: cl100k_base tokenizer not available",
        ))
    });

/// Token budget manager using tiktoken for counting.
pub struct TokenBudgeter {
    /// BPE tokenizer (cl100k_base for GPT-4/Claude)
    tokenizer: CoreBPE,
}

impl TokenBudgeter {
    /// Get the shared global instance (recommended for production).
    ///
    /// This avoids repeated tokenizer initialization overhead.
    /// The instance is created lazily on first access.
    pub fn shared() -> Arc<TokenBudgeter> {
        Arc::clone(&GLOBAL_BUDGETER)
    }

    /// Create a new standalone instance (for testing or isolated use).
    pub fn new() -> Result<Self> {
        Self::new_internal()
    }

    /// Internal constructor used by both `new()` and `shared()`.
    fn new_internal() -> Result<Self> {
        let tokenizer = tiktoken_rs::cl100k_base().map_err(|e| RetrievalErr::ConfigError {
            field: "tokenizer".to_string(),
            cause: format!("Failed to load cl100k_base tokenizer: {e}"),
        })?;

        Ok(Self { tokenizer })
    }

    /// Count tokens in a string.
    pub fn count_tokens(&self, text: &str) -> i32 {
        self.tokenizer.encode_ordinary(text).len() as i32
    }

    /// Find the optimal number of symbols that fit within the token budget.
    ///
    /// Uses sampling-based estimation to narrow the binary search range,
    /// reducing iterations from ~10 to ~3-4.
    ///
    /// # Arguments
    /// * `ranked_symbols` - Symbols sorted by rank descending
    /// * `renderer` - Tree renderer for generating output
    /// * `max_tokens` - Maximum token budget
    ///
    /// # Returns
    /// Optimal count of symbols to include
    pub fn find_optimal_count(
        &self,
        ranked_symbols: &[RankedSymbol],
        renderer: &TreeRenderer,
        max_tokens: i32,
    ) -> i32 {
        if ranked_symbols.is_empty() {
            return 0;
        }

        let total_symbols = ranked_symbols.len() as i32;

        // Quick check: if all symbols fit, return all
        let full_content = renderer.render_symbols(ranked_symbols, total_symbols);
        let full_tokens = self.count_tokens(&full_content);
        if full_tokens <= max_tokens {
            return total_symbols;
        }

        // Target tolerance: allow up to 15% under budget
        let min_target = (max_tokens as f32 * 0.85) as i32;

        // Use sampling to estimate optimal count and narrow search range
        let estimate = self.estimate_initial_count(ranked_symbols, renderer, max_tokens);

        // Check if estimate is already good enough (within 85%-100% of budget)
        if estimate > 0 && estimate <= total_symbols {
            let estimate_content = renderer.render_symbols(ranked_symbols, estimate);
            let estimate_tokens = self.count_tokens(&estimate_content);
            if estimate_tokens >= min_target && estimate_tokens <= max_tokens {
                return estimate;
            }
        }

        // Narrow search range to ±30% of estimate
        let mut low = ((estimate as f32 * 0.7).max(1.0)) as i32;
        let mut high = ((estimate as f32 * 1.3).min(total_symbols as f32)) as i32;

        // Ensure valid range
        if low > high || estimate == 0 {
            low = 1;
            high = total_symbols;
        }

        let mut best_count = 0_i32;

        while low <= high {
            let mid = (low + high) / 2;

            if mid == 0 {
                low = 1;
                continue;
            }

            let content = renderer.render_symbols(ranked_symbols, mid);
            let tokens = self.count_tokens(&content);

            if tokens <= max_tokens {
                best_count = mid;
                // If we're within tolerance, no need to search further
                if tokens >= min_target {
                    break;
                }
                low = mid + 1;
            } else {
                high = mid - 1;
            }
        }

        best_count
    }

    /// Estimate initial count by sampling and tokenizing.
    ///
    /// Renders a sample of symbols to calculate average tokens per symbol,
    /// then estimates how many symbols fit within the budget.
    fn estimate_initial_count(
        &self,
        ranked_symbols: &[RankedSymbol],
        renderer: &TreeRenderer,
        max_tokens: i32,
    ) -> i32 {
        if ranked_symbols.is_empty() {
            return 0;
        }

        // Sample first N symbols (use 20 for better accuracy)
        let sample_size = ranked_symbols.len().min(20) as i32;
        if sample_size == 0 {
            return 0;
        }

        // Render and tokenize sample
        let sample_content = renderer.render_symbols(ranked_symbols, sample_size);
        let sample_tokens = self.count_tokens(&sample_content);

        if sample_tokens == 0 {
            return ranked_symbols.len() as i32;
        }

        // Calculate average tokens per symbol
        let avg_tokens_per_symbol = sample_tokens as f32 / sample_size as f32;

        // Estimate target count
        let estimate = (max_tokens as f32 / avg_tokens_per_symbol) as i32;

        // Clamp to valid range
        estimate.max(1).min(ranked_symbols.len() as i32)
    }
}

#[cfg(test)]
#[path = "budget.test.rs"]
mod tests;
