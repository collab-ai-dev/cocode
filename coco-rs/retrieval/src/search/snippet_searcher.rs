//! Snippet search integration for HybridSearcher.
//!
//! Provides `SnippetSearcher` that converts symbol search results to
//! `SearchResult` for RRF fusion with BM25 and vector search.

use std::sync::Arc;

use crate::error::Result;
use crate::storage::SnippetStorage;
use crate::storage::SqliteStore;
use crate::storage::StoredSnippet;
use crate::storage::SymbolQuery;
use crate::types::CodeChunk;
use crate::types::ScoreType;
use crate::types::SearchResult;

/// Snippet searcher for symbol-based code search.
///
/// Wraps `SnippetStorage` and converts results to `SearchResult`
/// for integration with `HybridSearcher`.
pub struct SnippetSearcher {
    storage: SnippetStorage,
    workspace: String,
}

impl SnippetSearcher {
    /// Create a new snippet searcher.
    pub fn new(db: Arc<SqliteStore>, workspace: &str) -> Self {
        Self {
            storage: SnippetStorage::new(db),
            workspace: workspace.to_string(),
        }
    }

    /// Search snippets using the query string.
    ///
    /// Parses the query for symbol-specific syntax (`type:`, `name:`)
    /// and returns results as `SearchResult` with rank-based scores.
    pub async fn search(&self, query: &str, limit: i32) -> Result<Vec<SearchResult>> {
        let parsed = SymbolQuery::parse(query);

        // If empty query, return empty results
        if parsed.is_empty() {
            return Ok(Vec::new());
        }

        let snippets = self
            .storage
            .search_fts(&self.workspace, &parsed, limit)
            .await?;
        Ok(self.snippets_to_results(snippets))
    }

    /// Search snippets by name pattern only.
    pub async fn search_by_name(&self, name: &str, limit: i32) -> Result<Vec<SearchResult>> {
        let snippets = self
            .storage
            .search_by_name(&self.workspace, name, limit)
            .await?;
        Ok(self.snippets_to_results(snippets))
    }

    /// Get raw snippets for a file (for symbol outline).
    pub async fn get_file_symbols(&self, filepath: &str) -> Result<Vec<StoredSnippet>> {
        let query = SymbolQuery::for_file(filepath);
        self.storage.search_fts(&self.workspace, &query, 1000).await
    }

    /// Convert stored snippets to search results.
    fn snippets_to_results(&self, snippets: Vec<StoredSnippet>) -> Vec<SearchResult> {
        snippets
            .into_iter()
            .enumerate()
            .map(|(i, s)| self.snippet_to_result(s, i))
            .collect()
    }

    /// Convert a single snippet to SearchResult.
    fn snippet_to_result(&self, snippet: StoredSnippet, rank: usize) -> SearchResult {
        // Use signature or name as content
        let content = snippet
            .signature
            .clone()
            .unwrap_or_else(|| snippet.name.clone());

        // Generate unique chunk ID
        let chunk_id = format!(
            "snippet:{}:{}:{}",
            snippet.workspace, snippet.filepath, snippet.start_line
        );

        // Detect language from filepath
        let language = detect_language_from_path(&snippet.filepath);

        SearchResult {
            chunk: CodeChunk {
                id: chunk_id,
                source_id: snippet.workspace.clone(),
                filepath: snippet.filepath,
                language,
                content,
                start_line: snippet.start_line,
                end_line: snippet.end_line,
                embedding: None,
                modified_time: None,
                workspace: snippet.workspace,
                content_hash: snippet.content_hash,
                indexed_at: 0,
                parent_symbol: None, // TODO: Extract from TagExtractor
                is_overview: false,  // Snippets are not overview chunks
            },
            // Rank-based score (1.0, 0.5, 0.33, ...)
            score: 1.0 / (rank as f32 + 1.0),
            score_type: ScoreType::Snippet,
            is_stale: None, // Not hydrated yet
        }
    }

    /// Check if a query should use snippet search.
    ///
    /// Returns true if the query contains symbol-specific syntax.
    pub fn should_use_snippet_search(query: &str) -> bool {
        let parsed = SymbolQuery::parse(query);
        parsed.is_symbol_query()
    }
}

/// Detect programming language from file extension.
fn detect_language_from_path(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|ext| match ext {
            "rs" => "rust",
            "go" => "go",
            "py" => "python",
            "java" => "java",
            "ts" | "tsx" => "typescript",
            "js" | "jsx" => "javascript",
            _ => ext,
        })
        .unwrap_or("text")
        .to_string()
}

#[cfg(test)]
#[path = "snippet_searcher.test.rs"]
mod tests;
