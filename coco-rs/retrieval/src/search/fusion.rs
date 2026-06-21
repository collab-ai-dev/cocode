//! Reciprocal Rank Fusion (RRF) for combining search results.
//!
//! RRF is a simple but effective method for combining ranked lists.
//! Score = Σ weight / (rank + k), where k is typically 60.
//!
//! Also includes recency decay for boosting recently modified files.

use std::collections::HashMap;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use super::constants::DEFAULT_RECENCY_HALF_LIFE_DAYS;
use super::constants::DEFAULT_RRF_K;
use super::constants::LN_2;
use super::constants::SECONDS_PER_DAY;
use crate::types::CodeChunk;
use crate::types::ScoreType;
use crate::types::SearchResult;

/// RRF fusion configuration.
#[derive(Debug, Clone)]
pub struct RrfConfig {
    /// RRF constant (typically 60).
    pub k: f32,
    /// Weight for BM25 results.
    pub bm25_weight: f32,
    /// Weight for vector results.
    pub vector_weight: f32,
    /// Weight for snippet/tag matches.
    pub snippet_weight: f32,
    /// Weight for recently edited files as a retrieval source.
    pub recent_weight: f32,
    /// Weight for recency boost (0.0 = disabled).
    /// This is different from recent_weight - this applies time-based decay to all results.
    pub recency_boost_weight: f32,
    /// Recency decay half-life in days.
    pub recency_half_life_days: f32,
}

impl Default for RrfConfig {
    fn default() -> Self {
        Self {
            k: DEFAULT_RRF_K,
            bm25_weight: 0.5,
            vector_weight: 0.3,
            snippet_weight: 0.0,
            recent_weight: 0.2, // 20% for recently edited files (matches SearchConfig)
            recency_boost_weight: 0.0, // Disabled by default
            recency_half_life_days: DEFAULT_RECENCY_HALF_LIFE_DAYS,
        }
    }
}

impl RrfConfig {
    /// Create a new RRF config with custom weights.
    pub fn new(bm25_weight: f32, vector_weight: f32, snippet_weight: f32) -> Self {
        Self {
            k: DEFAULT_RRF_K,
            bm25_weight,
            vector_weight,
            snippet_weight,
            recent_weight: 0.0,
            recency_boost_weight: 0.0,
            recency_half_life_days: DEFAULT_RECENCY_HALF_LIFE_DAYS,
        }
    }

    /// Create a new RRF config with all four source weights.
    pub fn with_all_weights(
        bm25_weight: f32,
        vector_weight: f32,
        snippet_weight: f32,
        recent_weight: f32,
    ) -> Self {
        Self {
            k: DEFAULT_RRF_K,
            bm25_weight,
            vector_weight,
            snippet_weight,
            recent_weight,
            recency_boost_weight: 0.0,
            recency_half_life_days: DEFAULT_RECENCY_HALF_LIFE_DAYS,
        }
    }

    /// Set the weight for recently edited files source.
    pub fn with_recent_weight(mut self, weight: f32) -> Self {
        self.recent_weight = weight;
        self
    }

    /// Enable recency boost with default half-life (7 days).
    /// This applies time-based decay to all results.
    pub fn with_recency_boost(mut self, weight: f32) -> Self {
        self.recency_boost_weight = weight;
        self
    }

    /// Enable recency boost with custom half-life.
    pub fn with_recency_boost_config(mut self, weight: f32, half_life_days: f32) -> Self {
        self.recency_boost_weight = weight;
        self.recency_half_life_days = half_life_days;
        self
    }

    /// Adjust weights for identifier-heavy queries.
    ///
    /// When the query looks like an identifier (function name, variable, etc.),
    /// boost snippet weight for exact symbol matching.
    pub fn for_identifier_query(mut self) -> Self {
        self.bm25_weight = 0.4;
        self.vector_weight = 0.2;
        self.snippet_weight = 0.3;
        self.recent_weight = 0.1;
        self
    }

    /// Adjust weights for symbol-specific queries.
    ///
    /// When the query contains `type:` or `name:` syntax, heavily boost
    /// snippet weight for symbol matching.
    pub fn for_symbol_query(mut self) -> Self {
        self.bm25_weight = 0.2;
        self.vector_weight = 0.1;
        self.snippet_weight = 0.6;
        self.recent_weight = 0.1;
        self
    }
}

/// Check if query contains symbol search syntax.
///
/// Returns true if the query contains `type:`, `name:`, `file:`, or `path:` prefixes.
pub fn has_symbol_syntax(query: &str) -> bool {
    query.contains("type:")
        || query.contains("name:")
        || query.contains("file:")
        || query.contains("path:")
}

/// Calculate recency score based on file modification time.
///
/// Returns a value between 0.0 and 1.0, where:
/// - 1.0 = modified today
/// - 0.5 = modified `half_life_days` ago
/// - Decays exponentially for older files
///
/// Returns 0.0 if `modified_time` is None or in the future.
pub fn recency_score(modified_time: Option<i64>, half_life_days: f32) -> f32 {
    let Some(mtime) = modified_time else {
        return 0.0;
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    if mtime > now {
        return 0.0; // Future timestamp
    }

    let age_seconds = (now - mtime) as f32;
    let age_days = age_seconds / SECONDS_PER_DAY;

    // Exponential decay: score = exp(-ln(2) * age / half_life)
    let decay_rate = LN_2 / half_life_days;
    (-decay_rate * age_days).exp()
}

/// Apply recency boost to search results.
///
/// Adds `recency_score * recency_boost_weight` to each result's score.
pub fn apply_recency_boost(results: &mut [SearchResult], config: &RrfConfig) {
    if config.recency_boost_weight <= 0.0 {
        return;
    }

    for result in results.iter_mut() {
        let boost = recency_score(result.chunk.modified_time, config.recency_half_life_days);
        result.score += boost * config.recency_boost_weight;
    }
}

/// Compute RRF score for a result at a given rank.
fn rrf_score(rank: i32, weight: f32, k: f32) -> f32 {
    weight / (rank as f32 + k)
}

/// RRF source with results and weight.
struct RrfSource<'a> {
    results: &'a [SearchResult],
    weight: f32,
}

/// Internal unified RRF fusion implementation.
fn fuse_sources(sources: &[RrfSource<'_>], k: f32, limit: i32) -> Vec<SearchResult> {
    let mut scores: HashMap<String, (f32, CodeChunk)> = HashMap::new();

    for source in sources {
        for (rank, result) in source.results.iter().enumerate() {
            let score = rrf_score(rank as i32, source.weight, k);
            scores
                .entry(result.chunk.id.clone())
                .and_modify(|(s, _)| *s += score)
                .or_insert((score, result.chunk.clone()));
        }
    }

    let mut results: Vec<_> = scores
        .into_iter()
        .map(|(_, (score, chunk))| SearchResult {
            chunk,
            score,
            score_type: ScoreType::Hybrid,
            is_stale: None,
        })
        .collect();

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit as usize);
    results
}

/// Fuse multiple ranked lists using RRF.
///
/// # Arguments
/// * `bm25_results` - Results from BM25 full-text search
/// * `vector_results` - Results from vector similarity search
/// * `snippet_results` - Results from exact symbol matching
/// * `config` - RRF configuration
/// * `limit` - Maximum results to return
///
/// # Returns
/// Fused and re-ranked results
pub fn fuse_results(
    bm25_results: &[SearchResult],
    vector_results: &[SearchResult],
    snippet_results: &[SearchResult],
    config: &RrfConfig,
    limit: i32,
) -> Vec<SearchResult> {
    fuse_sources(
        &[
            RrfSource {
                results: bm25_results,
                weight: config.bm25_weight,
            },
            RrfSource {
                results: vector_results,
                weight: config.vector_weight,
            },
            RrfSource {
                results: snippet_results,
                weight: config.snippet_weight,
            },
        ],
        config.k,
        limit,
    )
}

/// Fuse only BM25 and vector results (simpler variant).
pub fn fuse_bm25_vector(
    bm25_results: &[SearchResult],
    vector_results: &[SearchResult],
    config: &RrfConfig,
    limit: i32,
) -> Vec<SearchResult> {
    fuse_results(bm25_results, vector_results, &[], config, limit)
}

/// Fuse all four sources: BM25, vector, snippet, and recent.
///
/// This is the full multi-source retrieval function.
pub fn fuse_all_results(
    bm25_results: &[SearchResult],
    vector_results: &[SearchResult],
    snippet_results: &[SearchResult],
    recent_results: &[SearchResult],
    config: &RrfConfig,
    limit: i32,
) -> Vec<SearchResult> {
    tracing::trace!(
        bm25 = bm25_results.len(),
        vector = vector_results.len(),
        snippet = snippet_results.len(),
        recent = recent_results.len(),
        bm25_weight = config.bm25_weight,
        vector_weight = config.vector_weight,
        snippet_weight = config.snippet_weight,
        recent_weight = config.recent_weight,
        "RRF fusion started"
    );

    fuse_sources(
        &[
            RrfSource {
                results: bm25_results,
                weight: config.bm25_weight,
            },
            RrfSource {
                results: vector_results,
                weight: config.vector_weight,
            },
            RrfSource {
                results: snippet_results,
                weight: config.snippet_weight,
            },
            RrfSource {
                results: recent_results,
                weight: config.recent_weight,
            },
        ],
        config.k,
        limit,
    )
}

/// Detect if a query looks like an identifier.
///
/// Returns true if the query matches patterns like:
/// - Single word with underscores: `get_user_name`
/// - CamelCase: `getUserName`, `GetUserName`
/// - Single word without spaces
pub fn is_identifier_query(query: &str) -> bool {
    let trimmed = query.trim();

    // Empty or has spaces -> not an identifier
    if trimmed.is_empty() || trimmed.contains(' ') {
        return false;
    }

    // Contains underscore -> likely snake_case identifier
    if trimmed.contains('_') {
        return true;
    }

    // Check for camelCase or PascalCase
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.is_empty() {
        return false;
    }

    // First char should be a letter
    if !chars[0].is_alphabetic() {
        return false;
    }

    // Check for mixed case (camelCase/PascalCase)
    let has_upper = chars.iter().any(|c| c.is_uppercase());
    let has_lower = chars.iter().any(|c| c.is_lowercase());

    // If has both upper and lower, it's likely camelCase/PascalCase
    if has_upper && has_lower {
        return true;
    }

    // Single word all lowercase or all uppercase is still an identifier
    chars.iter().all(|c| c.is_alphanumeric())
}

#[cfg(test)]
#[path = "fusion.test.rs"]
mod tests;
