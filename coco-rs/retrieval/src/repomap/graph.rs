//! Dependency graph for repo map PageRank.
//!
//! Builds a directed graph of file dependencies based on symbol definitions
//! and references extracted via tree-sitter tags.

use std::collections::HashMap;
use std::collections::HashSet;

use petgraph::graph::DiGraph;
use petgraph::graph::NodeIndex;

use crate::tags::extractor::CodeTag;

/// Extract terms from an identifier by splitting on snake_case and camelCase boundaries.
///
/// Examples:
/// - "calculate_user_score" → ["calculate", "user", "score"]
/// - "calculateUserScore" → ["calculate", "User", "Score"]
/// - "getUserName" → ["get", "User", "Name"]
pub fn extract_terms(ident: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();

    for c in ident.chars() {
        if c == '_' {
            // snake_case boundary
            if !current.is_empty() {
                terms.push(current.to_lowercase());
                current.clear();
            }
        } else if c.is_uppercase() && !current.is_empty() {
            // camelCase boundary
            terms.push(current.to_lowercase());
            current.clear();
            current.push(c);
        } else {
            current.push(c);
        }
    }

    if !current.is_empty() {
        terms.push(current.to_lowercase());
    }

    // Filter out very short terms (< 3 chars) and common words
    terms
        .into_iter()
        .filter(|t| t.len() >= 3 && !is_common_term(t))
        .collect()
}

/// Check if a term is too common to be meaningful for matching.
fn is_common_term(term: &str) -> bool {
    matches!(
        term,
        "get" | "set" | "new" | "the" | "and" | "for" | "with" | "from" | "into"
    )
}

/// Calculate term match score between query terms and a symbol name.
///
/// Returns a score in [0, 1] range based on term overlap.
fn term_match_score(symbol: &str, query_terms: &HashSet<String>) -> f64 {
    if query_terms.is_empty() {
        return 0.0;
    }

    let symbol_terms: HashSet<String> = extract_terms(symbol).into_iter().collect();
    if symbol_terms.is_empty() {
        return 0.0;
    }

    // Count matching terms
    let matches = symbol_terms.intersection(query_terms).count();
    if matches == 0 {
        return 0.0;
    }

    // Score = matches / min(symbol_terms, query_terms) - rewards high overlap
    let denominator = symbol_terms.len().min(query_terms.len());
    matches as f64 / denominator as f64
}

/// Check if an identifier uses snake_case or camelCase naming convention.
///
/// Returns true if:
/// - Contains underscore (snake_case: `calculate_user_score`)
/// - Contains mixed case transitions (camelCase: `calculateUserScore`)
fn is_well_named(ident: &str) -> bool {
    // Must be at least 8 characters (aider's threshold)
    if ident.len() < 8 {
        return false;
    }

    // Check for snake_case (contains underscore, not at start/end)
    let has_underscore = ident
        .chars()
        .enumerate()
        .any(|(i, c)| c == '_' && i > 0 && i < ident.len() - 1);

    if has_underscore {
        return true;
    }

    // Check for camelCase (lowercase followed by uppercase)
    let chars: Vec<char> = ident.chars().collect();
    for i in 1..chars.len() {
        if chars[i - 1].is_lowercase() && chars[i].is_uppercase() {
            return true;
        }
    }

    false
}

/// Edge data for the dependency graph.
#[derive(Debug, Clone)]
pub struct EdgeData {
    /// Base weight for this edge
    pub weight: f64,
    /// Symbol name that created this edge
    pub symbol: String,
}

/// Dependency graph for PageRank-based file ranking.
///
/// Nodes are file paths, edges represent symbol references between files.
pub struct DependencyGraph {
    /// The underlying petgraph
    graph: DiGraph<String, EdgeData>,
    /// Map from filepath to node index
    node_indices: HashMap<String, NodeIndex>,
    /// Map from symbol name to defining files
    definitions: HashMap<String, Vec<(String, CodeTag)>>,
    /// Map from symbol name to referencing files
    references: HashMap<String, Vec<String>>,
}

impl DependencyGraph {
    /// Create a new empty dependency graph.
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            node_indices: HashMap::new(),
            definitions: HashMap::new(),
            references: HashMap::new(),
        }
    }

    /// Clear the graph for reuse.
    pub fn clear(&mut self) {
        self.graph.clear();
        self.node_indices.clear();
        self.definitions.clear();
        self.references.clear();
    }

    /// Get or create a node for a file path.
    fn get_or_create_node(&mut self, filepath: &str) -> NodeIndex {
        if let Some(&idx) = self.node_indices.get(filepath) {
            idx
        } else {
            let idx = self.graph.add_node(filepath.to_string());
            self.node_indices.insert(filepath.to_string(), idx);
            idx
        }
    }

    /// Add tags from a file to the graph.
    ///
    /// Separates definitions and references for later edge building.
    pub fn add_file_tags(&mut self, filepath: &str, tags: &[CodeTag]) {
        // Ensure node exists
        self.get_or_create_node(filepath);

        for tag in tags {
            if tag.is_definition {
                self.definitions
                    .entry(tag.name.clone())
                    .or_default()
                    .push((filepath.to_string(), tag.clone()));
            } else {
                self.references
                    .entry(tag.name.clone())
                    .or_default()
                    .push(filepath.to_string());
            }
        }
    }

    /// Build weighted edges based on symbol references.
    ///
    /// Edge direction: referencing file -> defining file
    /// (PageRank will flow importance to files that are referenced)
    ///
    /// # Arguments
    /// * `chat_files` - Files currently in chat context (get boost)
    /// * `mentioned_idents` - Identifiers mentioned by user (exact match boost)
    /// * `query_terms` - Extracted terms for fuzzy matching (BM25-like boost)
    /// * `chat_file_weight` - Multiplier for chat file edges (default 50x)
    /// * `mentioned_ident_weight` - Multiplier for exact mention match (default 10x)
    /// * `private_symbol_weight` - Penalty for private symbols (default 0.1x)
    /// * `naming_style_weight` - Boost for well-named symbols (default 10x)
    /// * `term_match_weight` - Max boost for term matching (default 5x)
    #[allow(clippy::too_many_arguments)]
    pub fn build_edges(
        &mut self,
        chat_files: &HashSet<String>,
        mentioned_idents: &HashSet<String>,
        query_terms: &HashSet<String>,
        chat_file_weight: f32,
        mentioned_ident_weight: f32,
        private_symbol_weight: f32,
        naming_style_weight: f32,
        term_match_weight: f32,
    ) {
        // First, collect all edges to create (to avoid borrow conflicts)
        let mut edges_to_add: Vec<(String, String, f64, String)> = Vec::new();

        // For each symbol with both definitions and references
        for (symbol, def_files) in &self.definitions {
            let ref_files = match self.references.get(symbol) {
                Some(refs) => refs,
                None => continue, // No references to this symbol
            };

            // Calculate symbol-level weight multipliers
            let mut weight = 1.0_f64;

            // Private symbol penalty (underscore prefix)
            if symbol.starts_with('_') {
                weight *= private_symbol_weight as f64;
            }

            // Mentioned identifier boost (exact match)
            if mentioned_idents.contains(symbol) {
                weight *= mentioned_ident_weight as f64;
            } else if !query_terms.is_empty() {
                // Fuzzy term matching (BM25-like) - only if no exact match
                let match_score = term_match_score(symbol, query_terms);
                if match_score > 0.0 {
                    // Scale boost proportionally: full match = term_match_weight, partial = fraction
                    weight *= 1.0 + (term_match_weight as f64 - 1.0) * match_score;
                }
            }

            // Well-named identifier boost (snake_case/camelCase with len >= 8)
            // These are more specific and intentional names, less likely to be noise
            if is_well_named(symbol) {
                weight *= naming_style_weight as f64;
            }

            // Multi-defined penalty (>5 files = utility/noise)
            if def_files.len() > 5 {
                weight *= 0.1;
            }

            // High-frequency reference dampening (sqrt)
            if ref_files.len() > 10 {
                weight *= (10.0_f64 / ref_files.len() as f64).sqrt();
            }

            // Collect edges from referencing files to defining files
            for ref_file in ref_files {
                for (def_file, _tag) in def_files {
                    // Skip self-references
                    if ref_file == def_file {
                        continue;
                    }

                    // Apply chat file boost to edges from chat files
                    let mut edge_weight = weight;
                    if chat_files.contains(ref_file) {
                        edge_weight *= chat_file_weight as f64;
                    }

                    edges_to_add.push((
                        ref_file.clone(),
                        def_file.clone(),
                        edge_weight,
                        symbol.clone(),
                    ));
                }
            }
        }

        // Now add all edges to the graph
        // IMPORTANT: Check if edge already exists and accumulate weight instead of overwriting
        for (ref_file, def_file, edge_weight, symbol) in edges_to_add {
            let ref_idx = self.get_or_create_node(&ref_file);
            let def_idx = self.get_or_create_node(&def_file);

            if let Some(edge_idx) = self.graph.find_edge(ref_idx, def_idx) {
                // Edge exists: accumulate weight and track additional symbols
                if let Some(existing) = self.graph.edge_weight_mut(edge_idx) {
                    existing.weight += edge_weight;
                    // Track multiple symbols contributing to this edge
                    if !existing.symbol.contains(&symbol) {
                        existing.symbol.push(',');
                        existing.symbol.push_str(&symbol);
                    }
                }
            } else {
                // New edge: add it
                self.graph.add_edge(
                    ref_idx,
                    def_idx,
                    EdgeData {
                        weight: edge_weight,
                        symbol,
                    },
                );
            }
        }
    }

    /// Build personalization vector for PageRank.
    ///
    /// Chat files get higher initial probability.
    pub fn build_personalization(&self, chat_files: &HashSet<String>) -> HashMap<String, f64> {
        let mut personalization = HashMap::new();
        let node_count = self.graph.node_count();

        if node_count == 0 {
            return personalization;
        }

        // Default uniform distribution
        let default_prob = 1.0 / node_count as f64;

        // Chat files get 50x boost in initial probability
        let chat_boost = 50.0;
        let chat_count = chat_files.len();
        let non_chat_count = node_count.saturating_sub(chat_count);

        // Calculate probabilities that sum to 1.0
        let (chat_prob, non_chat_prob) = if chat_count > 0 && non_chat_count > 0 {
            // Total probability = chat_count * chat_prob + non_chat_count * non_chat_prob = 1.0
            // With chat_prob = chat_boost * non_chat_prob
            let non_chat_prob = 1.0 / (chat_count as f64 * chat_boost + non_chat_count as f64);
            let chat_prob = chat_boost * non_chat_prob;
            (chat_prob, non_chat_prob)
        } else if chat_count > 0 {
            (1.0 / chat_count as f64, 0.0)
        } else {
            (0.0, default_prob)
        };

        for filepath in self.node_indices.keys() {
            let prob = if chat_files.contains(filepath) {
                chat_prob
            } else {
                non_chat_prob
            };
            personalization.insert(filepath.clone(), prob);
        }

        personalization
    }

    /// Get the underlying graph for PageRank computation.
    pub fn graph(&self) -> &DiGraph<String, EdgeData> {
        &self.graph
    }

    /// Get the definitions map for symbol lookup.
    pub fn definitions(&self) -> &HashMap<String, Vec<(String, CodeTag)>> {
        &self.definitions
    }

    /// Compute total definition count per file.
    ///
    /// Returns a map from filepath to the total number of symbol definitions in that file.
    /// Used for distributing file ranks to symbols proportionally.
    pub fn compute_file_definition_counts(&self) -> HashMap<String, i32> {
        let mut counts: HashMap<String, i32> = HashMap::new();
        for def_locations in self.definitions.values() {
            for (filepath, _) in def_locations {
                *counts.entry(filepath.clone()).or_default() += 1;
            }
        }
        counts
    }

    /// Get file count in the graph.
    pub fn file_count(&self) -> usize {
        self.node_indices.len()
    }

    /// Get edge count in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "graph.test.rs"]
mod tests;
