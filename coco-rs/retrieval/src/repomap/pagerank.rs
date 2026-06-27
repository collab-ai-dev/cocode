//! PageRank algorithm for repo map file ranking.
//!
//! Implements personalized PageRank with weighted edges for
//! context-aware file importance ranking.

use std::collections::HashMap;

use petgraph::graph::DiGraph;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;

use crate::error::Result;
use crate::tags::extractor::CodeTag;

use super::RankedSymbol;
use super::graph::EdgeData;

/// PageRank ranker for file importance scoring.
pub struct PageRanker {
    /// Damping factor (typically 0.85)
    damping_factor: f64,
    /// Maximum iterations before stopping
    max_iterations: i32,
    /// Convergence tolerance
    tolerance: f64,
}

impl PageRanker {
    /// Create a new PageRanker with the given parameters.
    pub fn new(damping_factor: f64, max_iterations: i32, tolerance: f64) -> Self {
        Self {
            damping_factor,
            max_iterations,
            tolerance,
        }
    }

    /// Run personalized PageRank on the graph.
    ///
    /// Returns a map from file path to rank score.
    pub fn rank(
        &self,
        graph: &DiGraph<String, EdgeData>,
        personalization: &HashMap<String, f64>,
    ) -> Result<HashMap<String, f64>> {
        let node_count = graph.node_count();
        if node_count == 0 {
            return Ok(HashMap::new());
        }

        // Build node index to filepath mapping
        let mut idx_to_path: HashMap<NodeIndex, String> = HashMap::new();
        let mut path_to_idx: HashMap<String, NodeIndex> = HashMap::new();

        for idx in graph.node_indices() {
            let path = &graph[idx];
            idx_to_path.insert(idx, path.clone());
            path_to_idx.insert(path.clone(), idx);
        }

        // Initialize ranks
        let initial_rank = 1.0 / node_count as f64;
        let mut ranks: HashMap<NodeIndex, f64> = graph
            .node_indices()
            .map(|idx| (idx, initial_rank))
            .collect();

        // Precompute outgoing edge weights for each node
        let mut out_weights: HashMap<NodeIndex, f64> = HashMap::new();
        for idx in graph.node_indices() {
            let weight_sum: f64 = graph.edges(idx).map(|e| e.weight().weight).sum();
            out_weights.insert(idx, weight_sum);
        }

        // Personalization vector (default to uniform if not provided)
        let pers_vec: HashMap<NodeIndex, f64> = if personalization.is_empty() {
            graph
                .node_indices()
                .map(|idx| (idx, initial_rank))
                .collect()
        } else {
            personalization
                .iter()
                .filter_map(|(path, prob)| path_to_idx.get(path).map(|&idx| (idx, *prob)))
                .collect()
        };

        // Power iteration
        for _iteration in 0..self.max_iterations {
            let mut new_ranks: HashMap<NodeIndex, f64> = HashMap::new();
            let mut diff = 0.0_f64;

            for idx in graph.node_indices() {
                // Sum contributions from incoming edges
                let mut rank_sum = 0.0_f64;

                for edge in graph.edges_directed(idx, petgraph::Direction::Incoming) {
                    let source = edge.source();
                    let edge_weight = edge.weight().weight;
                    let source_out_weight = out_weights.get(&source).copied().unwrap_or(1.0);

                    if source_out_weight > 0.0 {
                        let source_rank = ranks.get(&source).copied().unwrap_or(initial_rank);
                        rank_sum += source_rank * (edge_weight / source_out_weight);
                    }
                }

                // Apply damping and personalization
                let pers_prob = pers_vec.get(&idx).copied().unwrap_or(initial_rank);
                let new_rank =
                    (1.0 - self.damping_factor) * pers_prob + self.damping_factor * rank_sum;

                let old_rank = ranks.get(&idx).copied().unwrap_or(initial_rank);
                diff += (new_rank - old_rank).abs();

                new_ranks.insert(idx, new_rank);
            }

            ranks = new_ranks;

            // Check convergence
            if diff < self.tolerance {
                break;
            }
        }

        // Normalize ranks to sum to 1.0
        let total: f64 = ranks.values().sum();
        if total > 0.0 {
            for rank in ranks.values_mut() {
                *rank /= total;
            }
        }

        // Convert back to filepath keys
        let result: HashMap<String, f64> = ranks
            .into_iter()
            .filter_map(|(idx, rank)| idx_to_path.get(&idx).map(|path| (path.clone(), rank)))
            .collect();

        Ok(result)
    }

    /// Distribute file ranks to individual symbol definitions.
    ///
    /// Returns a vector of (filepath, symbol_name, rank) sorted by rank descending.
    ///
    /// # Arguments
    /// * `file_ranks` - PageRank scores for each file
    /// * `definitions` - Map from symbol name to list of (filepath, tag)
    /// * `file_def_counts` - Total definition count per file (from graph.compute_file_definition_counts)
    pub fn distribute_to_definitions(
        &self,
        file_ranks: &HashMap<String, f64>,
        definitions: &HashMap<String, Vec<(String, CodeTag)>>,
        file_def_counts: &HashMap<String, i32>,
    ) -> Vec<RankedSymbol> {
        let mut ranked_symbols = Vec::new();

        // Collect all definitions with their file ranks
        for (symbol_name, def_locations) in definitions {
            for (filepath, tag) in def_locations {
                let file_rank = file_ranks.get(filepath).copied().unwrap_or(0.0);

                // Distribute file rank proportionally to total definitions in file
                // This ensures symbols in files with many definitions get smaller individual ranks
                let total_defs = file_def_counts.get(filepath).copied().unwrap_or(1);
                let symbol_rank = file_rank / total_defs.max(1) as f64;

                ranked_symbols.push(RankedSymbol {
                    tag: CodeTag {
                        name: symbol_name.clone(),
                        kind: tag.kind,
                        start_line: tag.start_line,
                        end_line: tag.end_line,
                        start_byte: tag.start_byte,
                        end_byte: tag.end_byte,
                        signature: tag.signature.clone(),
                        docs: tag.docs.clone(),
                        is_definition: true,
                    },
                    rank: symbol_rank,
                    filepath: filepath.clone(),
                });
            }
        }

        // Sort by rank descending
        ranked_symbols.sort_by(|a, b| {
            b.rank
                .partial_cmp(&a.rank)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        ranked_symbols
    }
}

impl Default for PageRanker {
    fn default() -> Self {
        Self::new(0.85, 100, 1e-6)
    }
}

#[cfg(test)]
#[path = "pagerank.test.rs"]
mod tests;
