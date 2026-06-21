//! Result deduplication for search results.
//!
//! Merges overlapping code chunks from the same file, keeping the highest score.
//! Reference: Continue `core/autocomplete/context/ranking/index.ts:70-131`

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::types::SearchResult;

/// Deduplicate search results by handling overlapping chunks from the same file.
///
/// # Algorithm
/// 1. Group results by filepath
/// 2. Sort each group by start_line
/// 3. For overlapping ranges (where prev.end_line >= next.start_line):
///    - Keep the chunk with larger line coverage (more complete code)
///    - If same coverage, keep the one with higher score
///    - Extend the kept chunk's range to cover both
/// 4. Re-sort by score descending
///
/// # Design Decision
/// We don't attempt to merge content from overlapping chunks because:
/// - Line-based content merging is error-prone and may corrupt code structure
/// - Keeping complete chunks ensures syntactic integrity
/// - The chunk with larger coverage typically contains more context
pub fn deduplicate_results(results: Vec<SearchResult>) -> Vec<SearchResult> {
    if results.is_empty() {
        return results;
    }

    // 1. Group by filepath
    let mut groups: HashMap<String, Vec<SearchResult>> = HashMap::new();
    for r in results {
        groups.entry(r.chunk.filepath.clone()).or_default().push(r);
    }

    // 2. Sort and merge each group
    let mut merged = Vec::new();
    for (_, mut group) in groups {
        if group.is_empty() {
            continue;
        }

        // Sort by start_line
        group.sort_by_key(|r| r.chunk.start_line);

        let mut current = group.remove(0);
        for next in group {
            if current.chunk.end_line >= next.chunk.start_line {
                // Overlapping chunks detected
                let current_lines = current.chunk.end_line - current.chunk.start_line + 1;
                let next_lines = next.chunk.end_line - next.chunk.start_line + 1;

                // Calculate merged range
                let merged_start = current.chunk.start_line.min(next.chunk.start_line);
                let merged_end = current.chunk.end_line.max(next.chunk.end_line);

                // Decide which chunk's content to keep:
                // - Prefer larger coverage (more complete code)
                // - If equal coverage, prefer higher score
                let keep_next = if next_lines > current_lines {
                    true
                } else if next_lines == current_lines {
                    next.score > current.score
                } else {
                    false
                };

                // Get the max score before potentially moving next
                let max_score = current.score.max(next.score);

                if keep_next {
                    // Use next's content but extend line range to cover both
                    current = next;
                }

                // Extend range to cover both chunks
                current.chunk.start_line = merged_start;
                current.chunk.end_line = merged_end;
                current.score = max_score;
            } else {
                merged.push(current);
                current = next;
            }
        }
        merged.push(current);
    }

    // 3. Re-sort by score descending
    merged.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

    merged
}
/// Deduplicate results with a configurable overlap threshold.
///
/// Two chunks are considered overlapping if they share at least `min_overlap_lines` lines.
pub fn deduplicate_with_threshold(
    results: Vec<SearchResult>,
    min_overlap_lines: i32,
) -> Vec<SearchResult> {
    if results.is_empty() || min_overlap_lines < 1 {
        return results;
    }

    // Group by filepath
    let mut groups: HashMap<String, Vec<SearchResult>> = HashMap::new();
    for r in results {
        groups.entry(r.chunk.filepath.clone()).or_default().push(r);
    }

    let mut merged = Vec::new();
    for (_, mut group) in groups {
        if group.is_empty() {
            continue;
        }

        group.sort_by_key(|r| r.chunk.start_line);

        let mut current = group.remove(0);
        for next in group {
            // Calculate overlap
            let overlap_start = current.chunk.start_line.max(next.chunk.start_line);
            let overlap_end = current.chunk.end_line.min(next.chunk.end_line);
            let overlap_lines = (overlap_end - overlap_start + 1).max(0);

            if overlap_lines >= min_overlap_lines {
                // Merge
                current.chunk.end_line = current.chunk.end_line.max(next.chunk.end_line);
                current.score = current.score.max(next.score);
            } else {
                merged.push(current);
                current = next;
            }
        }
        merged.push(current);
    }

    merged.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

    merged
}

/// Limit the number of chunks per file to ensure result diversity.
///
/// This prevents a single highly-relevant file from dominating all search results,
/// which improves context diversity for LLM consumption.
///
/// Reference: Tabby's `services/code.rs` (max 2 chunks per file)
///
/// # Arguments
/// * `results` - Search results (should already be sorted by score descending)
/// * `max_per_file` - Maximum number of chunks allowed per file
///
/// # Returns
/// Filtered results with at most `max_per_file` chunks per file,
/// maintaining the original score order.
pub fn limit_chunks_per_file(results: Vec<SearchResult>, max_per_file: usize) -> Vec<SearchResult> {
    if max_per_file == 0 {
        return Vec::new();
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    results
        .into_iter()
        .filter(|r| {
            let count = counts.entry(r.chunk.filepath.clone()).or_insert(0);
            if *count < max_per_file {
                *count += 1;
                true
            } else {
                false
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "dedup.test.rs"]
mod tests;
