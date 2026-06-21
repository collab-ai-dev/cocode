//! Cosine similarity for embeddings.
//!
//! This module provides functions for computing cosine similarity
//! between vectors, commonly used for embedding comparisons.

/// Compute cosine similarity between two vectors.
///
/// Returns a value between -1 and 1, where 1 means identical direction.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let magnitude_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let magnitude_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if magnitude_a == 0.0 || magnitude_b == 0.0 {
        return None;
    }

    Some(dot_product / (magnitude_a * magnitude_b))
}

/// Compute cosine similarity between two vectors, handling NaN and infinity.
pub fn safe_cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    cosine_similarity(a, b).unwrap_or(0.0)
}

/// Find the most similar vectors to a query vector.
///
/// Returns indices sorted by similarity (descending).
pub fn find_most_similar(
    query: &[f32],
    candidates: &[Vec<f32>],
    top_k: usize,
) -> Vec<(usize, f32)> {
    let mut similarities: Vec<(usize, f32)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(idx, candidate)| cosine_similarity(query, candidate).map(|sim| (idx, sim)))
        .collect();

    similarities.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    similarities.into_iter().take(top_k).collect()
}

#[cfg(test)]
#[path = "cosine_similarity.test.rs"]
mod tests;
