//! Rule-based reranking for search results.
//!
//! Applies boost factors based on:
//! - Exact match: query terms found in content
//! - Path relevance: query terms found in file path
//! - Recency: recently modified files
//!
//! No external models or APIs required. Fast and deterministic.

use async_trait::async_trait;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use super::Reranker;
use super::RerankerCapabilities;
use crate::config::RerankerConfig;
use crate::error::Result;
use crate::types::SearchResult;

/// Configuration for rule-based reranker.
#[derive(Debug, Clone)]
pub struct RuleBasedRerankerConfig {
    /// Boost multiplier when query terms are found exactly in content.
    pub exact_match_boost: f32,
    /// Boost multiplier when query terms appear in file path.
    pub path_relevance_boost: f32,
    /// Boost multiplier for recently modified files (< 7 days).
    pub recency_boost: f32,
    /// Days threshold for recency boost.
    pub recency_days_threshold: i32,
}

impl Default for RuleBasedRerankerConfig {
    fn default() -> Self {
        Self {
            exact_match_boost: 2.0,
            path_relevance_boost: 1.5,
            recency_boost: 1.2,
            recency_days_threshold: 7,
        }
    }
}

impl From<RerankerConfig> for RuleBasedRerankerConfig {
    fn from(config: RerankerConfig) -> Self {
        Self {
            exact_match_boost: config.exact_match_boost,
            path_relevance_boost: config.path_relevance_boost,
            recency_boost: config.recency_boost,
            recency_days_threshold: config.recency_days_threshold,
        }
    }
}

/// Rule-based reranker.
///
/// Applies configurable boost factors to search results based on
/// exact matches, path relevance, and file recency.
#[derive(Debug, Clone)]
pub struct RuleBasedReranker {
    config: RuleBasedRerankerConfig,
}

impl RuleBasedReranker {
    /// Create a new rule-based reranker with default config.
    pub fn new() -> Self {
        Self {
            config: RuleBasedRerankerConfig::default(),
        }
    }

    /// Create a new rule-based reranker with custom config.
    pub fn with_config(config: RuleBasedRerankerConfig) -> Self {
        Self { config }
    }

    /// Check if content contains all query terms (case-insensitive).
    fn contains_exact_match(&self, content: &str, query: &str) -> bool {
        let content_lower = content.to_lowercase();
        let query_terms: Vec<&str> = query.split_whitespace().collect();

        // All query terms must be present
        query_terms
            .iter()
            .all(|term| content_lower.contains(&term.to_lowercase()))
    }

    /// Check if file path contains any query terms.
    fn path_contains_query_terms(&self, filepath: &str, query: &str) -> bool {
        let filepath_lower = filepath.to_lowercase();
        let query_terms: Vec<&str> = query.split_whitespace().collect();

        // Any query term in path is a match
        query_terms
            .iter()
            .any(|term| filepath_lower.contains(&term.to_lowercase()))
    }

    /// Calculate age in days from Unix timestamp.
    fn age_in_days(&self, modified_time: Option<i64>) -> Option<i64> {
        let mtime = modified_time?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
        Some((now - mtime) / 86400)
    }
}

impl Default for RuleBasedReranker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Reranker for RuleBasedReranker {
    fn name(&self) -> &str {
        "rule_based"
    }

    fn capabilities(&self) -> RerankerCapabilities {
        RerankerCapabilities {
            requires_network: false,
            supports_batch: true,
            max_batch_size: None,
            is_async: false,
        }
    }

    async fn rerank(&self, query: &str, results: &mut [SearchResult]) -> Result<()> {
        if results.is_empty() || query.is_empty() {
            return Ok(());
        }

        for result in results.iter_mut() {
            let mut boost = 1.0_f32;

            // 1. Exact match boost - query terms in content
            if self.contains_exact_match(&result.chunk.content, query) {
                boost *= self.config.exact_match_boost;
            }

            // 2. Path relevance - query terms in filepath
            if self.path_contains_query_terms(&result.chunk.filepath, query) {
                boost *= self.config.path_relevance_boost;
            }

            // 3. Recency boost - recently modified files
            if let Some(age_days) = self.age_in_days(result.chunk.modified_time)
                && age_days < self.config.recency_days_threshold as i64
            {
                boost *= self.config.recency_boost;
            }

            result.score *= boost;
        }

        // Re-sort by adjusted scores (descending)
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(())
    }
}

#[cfg(test)]
#[path = "rule_based.test.rs"]
mod tests;
