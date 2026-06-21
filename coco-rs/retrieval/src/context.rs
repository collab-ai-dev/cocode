//! Shared retrieval context.
//!
//! Provides shared state for all retrieval services.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::RetrievalConfig;
use crate::error::Result;
use crate::indexing::FilterSummary;
use crate::repomap::TokenBudgeter;
use crate::storage::SqliteStore;
use crate::storage::SqliteVecStore;
use crate::storage::VectorStore;

/// Feature flags for retrieval system.
#[derive(Debug, Clone, Copy, Default)]
pub struct RetrievalFeatures {
    /// Enable BM25 full-text search (basic code search).
    pub code_search: bool,
    /// Enable vector similarity search.
    pub vector_search: bool,
    /// Enable query rewriting (CN/EN translation, expansion).
    pub query_rewrite: bool,
}

impl RetrievalFeatures {
    // ========== Presets (const) ==========

    /// No features enabled.
    pub const NONE: Self = Self {
        code_search: false,
        vector_search: false,
        query_rewrite: false,
    };

    /// Minimal features for testing (BM25 only).
    pub const MINIMAL: Self = Self {
        code_search: true,
        vector_search: false,
        query_rewrite: false,
    };

    /// Standard features for production (BM25 + query rewrite).
    pub const STANDARD: Self = Self {
        code_search: true,
        vector_search: false,
        query_rewrite: true,
    };

    /// Full features with vector search.
    pub const FULL: Self = Self {
        code_search: true,
        vector_search: true,
        query_rewrite: true,
    };

    // ========== Factory Methods ==========

    /// Create with all features disabled.
    pub fn none() -> Self {
        Self::NONE
    }

    /// Create with code search enabled.
    pub fn with_code_search() -> Self {
        Self::MINIMAL
    }

    /// Enable all features.
    pub fn all() -> Self {
        Self::FULL
    }

    // ========== Utility Methods ==========

    /// Check if any search feature is enabled.
    pub fn has_search(&self) -> bool {
        self.code_search || self.vector_search
    }
}

/// Shared context for all retrieval services.
///
/// Holds configuration and shared resources that are used across
/// `SearchService`, `IndexService`, `RepoMapService`, etc.
pub struct RetrievalContext {
    /// Configuration loaded from retrieval.toml.
    config: Arc<RetrievalConfig>,
    /// Feature flags.
    features: RetrievalFeatures,
    /// Workspace root directory.
    workspace_root: PathBuf,
    /// SQLite store for metadata and tags.
    db: Arc<SqliteStore>,
    /// Token budgeter for repomap (singleton).
    budgeter: Arc<TokenBudgeter>,
    /// Vector store (sqlite-vec backed).
    vector_store: Arc<dyn VectorStore>,
}

impl RetrievalContext {
    /// Create a new retrieval context.
    ///
    /// Initializes shared resources (db, budgeter, vector_store) eagerly.
    pub async fn new(
        config: RetrievalConfig,
        features: RetrievalFeatures,
        workspace_root: PathBuf,
    ) -> Result<Self> {
        // Initialize SQLite store
        let db_path = config.data_dir.join("retrieval.db");
        let db = Arc::new(SqliteStore::open(&db_path)?);

        // Get shared token budgeter (global singleton)
        let budgeter = TokenBudgeter::shared();

        // Initialize vector store (sqlite-vec)
        let vector_store: Arc<dyn VectorStore> = Arc::new(SqliteVecStore::open(&config.data_dir)?);

        Ok(Self {
            config: Arc::new(config),
            features,
            workspace_root,
            db,
            budgeter,
            vector_store,
        })
    }

    /// Get configuration reference.
    pub fn config(&self) -> &RetrievalConfig {
        &self.config
    }

    /// Get configuration Arc for cloning.
    pub fn config_arc(&self) -> Arc<RetrievalConfig> {
        Arc::clone(&self.config)
    }

    /// Get feature flags.
    pub fn features(&self) -> &RetrievalFeatures {
        &self.features
    }

    /// Get workspace root path.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Get workspace name (directory name).
    pub fn workspace_name(&self) -> &str {
        self.workspace_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("default")
    }

    /// Get shared SQLite store.
    pub fn db(&self) -> Arc<SqliteStore> {
        Arc::clone(&self.db)
    }

    /// Get shared token budgeter.
    pub fn budgeter(&self) -> Arc<TokenBudgeter> {
        Arc::clone(&self.budgeter)
    }

    /// Get shared vector store.
    pub fn vector_store(&self) -> Arc<dyn VectorStore> {
        Arc::clone(&self.vector_store)
    }

    /// Get the file filter summary for event emission.
    pub fn filter_summary(&self) -> Option<FilterSummary> {
        let summary = FilterSummary {
            include_dirs: self.config.indexing.include_dirs.clone(),
            exclude_dirs: self.config.indexing.exclude_dirs.clone(),
            include_extensions: self.config.indexing.include_extensions.clone(),
            exclude_extensions: self.config.indexing.exclude_extensions.clone(),
        };
        if summary.has_filters() {
            Some(summary)
        } else {
            None
        }
    }
}

impl std::fmt::Debug for RetrievalContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetrievalContext")
            .field("workspace_root", &self.workspace_root)
            .field("features", &self.features)
            .finish()
    }
}

#[cfg(test)]
#[path = "context.test.rs"]
mod tests;
