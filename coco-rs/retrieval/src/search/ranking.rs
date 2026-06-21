//! Jaccard similarity ranking for search results.
//!
//! Provides symbol-level similarity calculation for better ranking of code search results.
//! Reference: Continue `core/autocomplete/context/ranking/index.ts:6-36`

use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::types::SearchResult;

/// Regex for splitting code into symbols (tokens).
/// Splits on whitespace and common punctuation.
static SYMBOL_SPLIT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"[\s.,/#!$%^&*;:{}=\-_`~()\[\]<>"'\\|+@?]+"#).unwrap());

/// Extract symbols (tokens) from a code snippet.
///
/// Splits the text on whitespace and punctuation, converting to lowercase.
/// Returns a set of unique symbols.
pub fn extract_symbols(text: &str) -> HashSet<String> {
    SYMBOL_SPLIT
        .split(text)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

/// Calculate Jaccard similarity between two text snippets.
///
/// Jaccard similarity = |A ∩ B| / |A ∪ B|
///
/// Returns a value between 0.0 (no overlap) and 1.0 (identical symbols).
pub fn jaccard_similarity(a: &str, b: &str) -> f32 {
    let set_a = extract_symbols(a);
    let set_b = extract_symbols(b);

    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

/// Boost results based on Jaccard similarity with the query.
///
/// Adds `similarity * boost_factor` to each result's score.
pub fn apply_jaccard_boost(results: &mut [SearchResult], query: &str, boost_factor: f32) {
    for result in results.iter_mut() {
        let similarity = jaccard_similarity(query, &result.chunk.content);
        result.score += similarity * boost_factor;
    }
}

/// Re-rank results by Jaccard similarity.
///
/// Useful for tie-breaking when scores are similar.
pub fn rerank_by_jaccard(results: &mut [SearchResult], query: &str) {
    let query_symbols = extract_symbols(query);

    // Sort by: (original_score, jaccard_similarity) descending
    results.sort_by(|a, b| {
        let sim_a = jaccard_with_set(&a.chunk.content, &query_symbols);
        let sim_b = jaccard_with_set(&b.chunk.content, &query_symbols);

        // Compare by score first, then by Jaccard similarity
        match b.score.partial_cmp(&a.score) {
            Some(std::cmp::Ordering::Equal) => sim_b
                .partial_cmp(&sim_a)
                .unwrap_or(std::cmp::Ordering::Equal),
            Some(ord) => ord,
            None => std::cmp::Ordering::Equal,
        }
    });
}

/// Calculate Jaccard similarity with a pre-computed symbol set.
fn jaccard_with_set(text: &str, query_symbols: &HashSet<String>) -> f32 {
    let text_symbols = extract_symbols(text);

    let intersection = text_symbols.intersection(query_symbols).count();
    let union = text_symbols.union(query_symbols).count();

    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

#[cfg(test)]
#[path = "ranking.test.rs"]
mod tests;
