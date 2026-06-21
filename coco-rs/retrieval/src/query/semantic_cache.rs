//! In-memory semantic query cache using LRU + embedding similarity.
//!
//! Caches query results and looks them up by embedding similarity,
//! allowing semantically similar queries to reuse cached results.

use std::collections::VecDeque;
use std::sync::RwLock;

/// Configuration for semantic cache.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SemanticCacheConfig {
    /// Enable semantic similarity lookup.
    #[serde(default)]
    pub enabled: bool,

    /// Similarity threshold for cache hit (0.0-1.0).
    /// Higher values require more similar queries.
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f32,

    /// Maximum number of entries to cache.
    #[serde(default = "default_max_entries")]
    pub max_entries: usize,
}

fn default_similarity_threshold() -> f32 {
    0.95
}

fn default_max_entries() -> usize {
    1000
}

impl Default for SemanticCacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            similarity_threshold: default_similarity_threshold(),
            max_entries: default_max_entries(),
        }
    }
}

/// Cache entry with query embedding.
struct CacheEntry {
    /// Original query text (for debugging).
    #[allow(dead_code)]
    query: String,
    /// Query embedding vector.
    embedding: Vec<f32>,
    /// Cached result (serialized JSON or raw data).
    result: String,
}

/// In-memory semantic query cache.
///
/// Uses embedding similarity for lookup and LRU eviction.
/// No persistence - cleared on restart.
///
/// Thread-safe via RwLock.
pub struct SemanticQueryCache {
    entries: RwLock<VecDeque<CacheEntry>>,
    config: SemanticCacheConfig,
}

impl SemanticQueryCache {
    /// Create a new semantic cache with the given configuration.
    pub fn new(config: SemanticCacheConfig) -> Self {
        Self {
            entries: RwLock::new(VecDeque::with_capacity(config.max_entries)),
            config,
        }
    }

    /// Check if the cache is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Lookup by semantic similarity.
    ///
    /// Returns cached result if a query with similarity above threshold exists.
    /// Returns None if cache is disabled, empty, or no similar query found.
    pub fn get_semantic(&self, query_embedding: &[f32]) -> Option<String> {
        if !self.config.enabled {
            return None;
        }

        let entries = self
            .entries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let mut best_match: Option<(f32, &str)> = None;

        for entry in entries.iter() {
            let similarity = cosine_similarity(query_embedding, &entry.embedding);
            if similarity >= self.config.similarity_threshold {
                match &best_match {
                    Some((best_sim, _)) if similarity <= *best_sim => {}
                    _ => best_match = Some((similarity, &entry.result)),
                }
            }
        }

        best_match.map(|(_, result)| result.to_string())
    }

    /// Store query with its embedding.
    ///
    /// Uses LRU eviction when capacity is reached.
    pub fn put(&self, query: &str, embedding: Vec<f32>, result: &str) {
        if !self.config.enabled {
            return;
        }

        let mut entries = match self.entries.write() {
            Ok(e) => e,
            Err(_) => return,
        };

        // Evict oldest if at capacity
        if entries.len() >= self.config.max_entries {
            entries.pop_front();
        }

        entries.push_back(CacheEntry {
            query: query.to_string(),
            embedding,
            result: result.to_string(),
        });
    }

    /// Clear all cached entries.
    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.write() {
            entries.clear();
        }
    }

    /// Get current number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.read().map(|e| e.len()).unwrap_or(0)
    }

    /// Check if cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get cache hit statistics (for monitoring).
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.len(),
            capacity: self.config.max_entries,
            threshold: self.config.similarity_threshold,
        }
    }
}

/// Cache statistics for monitoring.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Current number of entries.
    pub entries: usize,
    /// Maximum capacity.
    pub capacity: usize,
    /// Similarity threshold.
    pub threshold: f32,
}

/// Compute cosine similarity between two embedding vectors.
///
/// Returns value between -1.0 and 1.0, where 1.0 means identical.
/// Returns 0.0 if either vector is zero-length or has zero magnitude.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

#[cfg(test)]
#[path = "semantic_cache.test.rs"]
mod tests;
