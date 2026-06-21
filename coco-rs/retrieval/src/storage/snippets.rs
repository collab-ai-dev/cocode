//! Snippet storage for extracted tags.
//!
//! Stores and retrieves code snippets (symbols) in SQLite.
//! Includes FTS5 full-text search on symbol names, signatures, and docs.

use std::sync::Arc;

use crate::error::Result;
use crate::storage::SqliteStore;
use crate::tags::CodeTag;
use crate::tags::TagKind;

// ============================================================================
// SymbolQuery - Parsed symbol search query
// ============================================================================

/// Parsed symbol query.
///
/// Supports syntax like:
/// - `type:function` - Filter by symbol type
/// - `name:parse` - Filter by name pattern
/// - `file:src/main.rs` or `path:src/main.rs` - Filter by file path
/// - `type:method name:get` - Combined filters
/// - `parse error` - Free-text search in signature/docs
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SymbolQuery {
    /// Symbol name pattern (without wildcards)
    pub name: Option<String>,
    /// Symbol type filter
    pub kind: Option<TagKind>,
    /// File path pattern
    pub filepath: Option<String>,
    /// Free-text search in signature/docs
    pub text: Option<String>,
}

impl SymbolQuery {
    /// Parse a query string.
    ///
    /// Syntax:
    /// - `type:function` - Filter by TagKind
    /// - `name:parse` - Filter by name (wildcards stripped)
    /// - `file:src/main.rs` or `path:src/main.rs` - Filter by filepath
    /// - Remaining terms become free-text search
    pub fn parse(query: &str) -> Self {
        let mut result = Self::default();
        let mut remaining = Vec::new();

        for part in query.split_whitespace() {
            if let Some(stripped) = part.strip_prefix("type:") {
                result.kind = Some(TagKind::from_syntax_type(stripped));
            } else if let Some(stripped) = part.strip_prefix("name:") {
                // Strip wildcards for SQL LIKE pattern
                result.name = Some(stripped.replace('*', ""));
            } else if let Some(stripped) = part
                .strip_prefix("file:")
                .or_else(|| part.strip_prefix("path:"))
            {
                result.filepath = Some(stripped.to_string());
            } else {
                remaining.push(part);
            }
        }

        if !remaining.is_empty() {
            result.text = Some(remaining.join(" "));
        }

        result
    }

    /// Create a query for a specific file.
    pub fn for_file(filepath: &str) -> Self {
        Self {
            filepath: Some(filepath.to_string()),
            ..Default::default()
        }
    }

    /// Check if this is a symbol-specific query (has type or name filter).
    pub fn is_symbol_query(&self) -> bool {
        self.name.is_some() || self.kind.is_some()
    }

    /// Check if query is empty (no filters or text).
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.kind.is_none() && self.text.is_none() && self.filepath.is_none()
    }
}

// ============================================================================
// Query Builders
// ============================================================================

/// Query result with SQL and parameters for parameterized execution.
struct ParameterizedQuery {
    sql: String,
    params: Vec<String>,
}

/// Build a query using FTS5 MATCH for text search.
fn build_fts_query(
    workspace: &str,
    name_pattern: &Option<String>,
    kind_filter: &Option<String>,
    filepath_filter: &Option<String>,
    text_query: &Option<String>,
    limit: i32,
) -> ParameterizedQuery {
    let mut conditions = Vec::new();
    let mut params = Vec::new();
    let mut param_idx = 1;

    // Workspace filter (always present)
    conditions.push(format!("s.workspace = ?{param_idx}"));
    params.push(workspace.to_string());
    param_idx += 1;

    // FTS5 match condition
    if let Some(text) = text_query {
        conditions.push(format!("snippets_fts MATCH ?{param_idx}"));
        // Wrap in quotes for FTS5 phrase search
        params.push(format!("\"{}\"", text.replace('"', "\"\"")));
        param_idx += 1;
    }

    // Name pattern (LIKE)
    if let Some(name) = name_pattern {
        conditions.push(format!("s.name LIKE ?{param_idx}"));
        params.push(format!("%{name}%"));
        param_idx += 1;
    }

    // Type filter
    if let Some(kind) = kind_filter {
        conditions.push(format!("s.syntax_type = ?{param_idx}"));
        params.push(kind.clone());
        param_idx += 1;
    }

    // Filepath filter (exact or LIKE match)
    if let Some(filepath) = filepath_filter {
        // Support both exact match and pattern match
        if filepath.contains('*') || filepath.contains('%') {
            conditions.push(format!("s.filepath LIKE ?{param_idx}"));
            params.push(filepath.replace('*', "%"));
        } else {
            conditions.push(format!("s.filepath = ?{param_idx}"));
            params.push(filepath.clone());
        }
        // param_idx += 1; // not needed, last param
    }

    let sql = format!(
        "SELECT s.id, s.workspace, s.filepath, s.name, s.syntax_type,
                s.start_line, s.end_line, s.signature, s.docs, s.content_hash
         FROM snippets s
         JOIN snippets_fts ON snippets_fts.rowid = s.id
         WHERE {}
         ORDER BY s.name
         LIMIT {limit}",
        conditions.join(" AND ")
    );

    ParameterizedQuery { sql, params }
}

/// Build a simple query without FTS5.
fn build_simple_query(
    workspace: &str,
    name_pattern: &Option<String>,
    kind_filter: &Option<String>,
    filepath_filter: &Option<String>,
    limit: i32,
) -> ParameterizedQuery {
    let mut conditions = Vec::new();
    let mut params = Vec::new();
    let mut param_idx = 1;

    // Workspace filter (always present)
    conditions.push(format!("workspace = ?{param_idx}"));
    params.push(workspace.to_string());
    param_idx += 1;

    // Name pattern (LIKE)
    if let Some(name) = name_pattern {
        conditions.push(format!("name LIKE ?{param_idx}"));
        params.push(format!("%{name}%"));
        param_idx += 1;
    }

    // Type filter
    if let Some(kind) = kind_filter {
        conditions.push(format!("syntax_type = ?{param_idx}"));
        params.push(kind.clone());
        param_idx += 1;
    }

    // Filepath filter (exact or LIKE match)
    if let Some(filepath) = filepath_filter {
        if filepath.contains('*') || filepath.contains('%') {
            conditions.push(format!("filepath LIKE ?{param_idx}"));
            params.push(filepath.replace('*', "%"));
        } else {
            conditions.push(format!("filepath = ?{param_idx}"));
            params.push(filepath.clone());
        }
        // param_idx += 1; // not needed, last param
    }

    let sql = format!(
        "SELECT id, workspace, filepath, name, syntax_type,
                start_line, end_line, signature, docs, content_hash
         FROM snippets
         WHERE {}
         ORDER BY name
         LIMIT {limit}",
        conditions.join(" AND ")
    );

    ParameterizedQuery { sql, params }
}

// ============================================================================
// StoredSnippet
// ============================================================================

/// Stored code snippet (symbol).
#[derive(Debug, Clone)]
pub struct StoredSnippet {
    /// Database ID
    pub id: i64,
    /// Workspace identifier
    pub workspace: String,
    /// File path
    pub filepath: String,
    /// Symbol name
    pub name: String,
    /// Syntax type (function, class, etc.)
    pub syntax_type: String,
    /// Start line (0-indexed)
    pub start_line: i32,
    /// End line (0-indexed)
    pub end_line: i32,
    /// Optional signature
    pub signature: Option<String>,
    /// Optional documentation
    pub docs: Option<String>,
    /// Content hash for deduplication
    pub content_hash: String,
}

/// Snippet storage service.
pub struct SnippetStorage {
    db: Arc<SqliteStore>,
}

impl SnippetStorage {
    /// Create a new snippet storage service.
    pub fn new(db: Arc<SqliteStore>) -> Self {
        Self { db }
    }

    /// Store a batch of tags as snippets.
    pub async fn store_tags(
        &self,
        workspace: &str,
        filepath: &str,
        tags: &[CodeTag],
        content_hash: &str,
    ) -> Result<i32> {
        let ws = workspace.to_string();
        let fp = filepath.to_string();
        let hash = content_hash.to_string();

        // Prepare data for insertion
        let snippets: Vec<_> = tags
            .iter()
            .map(|tag| {
                (
                    tag.name.clone(),
                    tag.kind.as_str().to_string(),
                    tag.start_line,
                    tag.end_line,
                    tag.signature.clone(),
                    tag.docs.clone(),
                )
            })
            .collect();

        self.db
            .transaction(move |conn| {
                // Delete existing snippets for this file
                conn.execute(
                    "DELETE FROM snippets WHERE workspace = ? AND filepath = ?",
                    rusqlite::params![ws, fp],
                )?;

                let mut count = 0;
                for (name, syntax_type, start_line, end_line, signature, docs) in snippets {
                    conn.execute(
                        "INSERT INTO snippets (workspace, filepath, name, syntax_type, start_line, end_line, signature, docs, content_hash)
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        rusqlite::params![ws, fp, name, syntax_type, start_line, end_line, signature, docs, hash],
                    )?;
                    count += 1;
                }

                Ok(count)
            })
            .await
    }

    /// Search snippets by symbol name.
    pub async fn search_by_name(
        &self,
        workspace: &str,
        query: &str,
        limit: i32,
    ) -> Result<Vec<StoredSnippet>> {
        let ws = workspace.to_string();
        let q = format!("%{query}%");
        let lim = limit;

        self.db
            .query(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, workspace, filepath, name, syntax_type, start_line, end_line, signature, docs, content_hash
                     FROM snippets
                     WHERE workspace = ? AND name LIKE ?
                     ORDER BY name
                     LIMIT ?",
                )?;

                let rows = stmt.query_map(rusqlite::params![ws, q, lim], |row| {
                    Ok(StoredSnippet {
                        id: row.get(0)?,
                        workspace: row.get(1)?,
                        filepath: row.get(2)?,
                        name: row.get(3)?,
                        syntax_type: row.get(4)?,
                        start_line: row.get(5)?,
                        end_line: row.get(6)?,
                        signature: row.get(7)?,
                        docs: row.get(8)?,
                        content_hash: row.get(9)?,
                    })
                })?;

                let mut results = Vec::new();
                for row in rows {
                    results.push(row?);
                }
                Ok(results)
            })
            .await
    }

    /// Search snippets by syntax type.
    pub async fn search_by_type(
        &self,
        workspace: &str,
        syntax_type: TagKind,
        limit: i32,
    ) -> Result<Vec<StoredSnippet>> {
        let ws = workspace.to_string();
        let st = syntax_type.as_str().to_string();
        let lim = limit;

        self.db
            .query(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, workspace, filepath, name, syntax_type, start_line, end_line, signature, docs, content_hash
                     FROM snippets
                     WHERE workspace = ? AND syntax_type = ?
                     ORDER BY name
                     LIMIT ?",
                )?;

                let rows = stmt.query_map(rusqlite::params![ws, st, lim], |row| {
                    Ok(StoredSnippet {
                        id: row.get(0)?,
                        workspace: row.get(1)?,
                        filepath: row.get(2)?,
                        name: row.get(3)?,
                        syntax_type: row.get(4)?,
                        start_line: row.get(5)?,
                        end_line: row.get(6)?,
                        signature: row.get(7)?,
                        docs: row.get(8)?,
                        content_hash: row.get(9)?,
                    })
                })?;

                let mut results = Vec::new();
                for row in rows {
                    results.push(row?);
                }
                Ok(results)
            })
            .await
    }

    /// Get snippets for a specific file.
    pub async fn get_by_filepath(
        &self,
        workspace: &str,
        filepath: &str,
    ) -> Result<Vec<StoredSnippet>> {
        let ws = workspace.to_string();
        let fp = filepath.to_string();

        self.db
            .query(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, workspace, filepath, name, syntax_type, start_line, end_line, signature, docs, content_hash
                     FROM snippets
                     WHERE workspace = ? AND filepath = ?
                     ORDER BY start_line",
                )?;

                let rows = stmt.query_map(rusqlite::params![ws, fp], |row| {
                    Ok(StoredSnippet {
                        id: row.get(0)?,
                        workspace: row.get(1)?,
                        filepath: row.get(2)?,
                        name: row.get(3)?,
                        syntax_type: row.get(4)?,
                        start_line: row.get(5)?,
                        end_line: row.get(6)?,
                        signature: row.get(7)?,
                        docs: row.get(8)?,
                        content_hash: row.get(9)?,
                    })
                })?;

                let mut results = Vec::new();
                for row in rows {
                    results.push(row?);
                }
                Ok(results)
            })
            .await
    }

    /// Delete snippets for a file.
    pub async fn delete_by_filepath(&self, workspace: &str, filepath: &str) -> Result<i32> {
        let ws = workspace.to_string();
        let fp = filepath.to_string();

        self.db
            .query(move |conn| {
                let count = conn.execute(
                    "DELETE FROM snippets WHERE workspace = ? AND filepath = ?",
                    rusqlite::params![ws, fp],
                )?;
                Ok(count as i32)
            })
            .await
    }

    /// Delete all snippets for a workspace.
    pub async fn delete_by_workspace(&self, workspace: &str) -> Result<i32> {
        let ws = workspace.to_string();

        self.db
            .query(move |conn| {
                let count = conn.execute(
                    "DELETE FROM snippets WHERE workspace = ?",
                    rusqlite::params![ws],
                )?;
                Ok(count as i32)
            })
            .await
    }

    /// Count total snippets in workspace.
    pub async fn count(&self, workspace: &str) -> Result<i64> {
        let ws = workspace.to_string();

        self.db
            .query(move |conn| {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM snippets WHERE workspace = ?",
                    [&ws],
                    |row| row.get(0),
                )?;
                Ok(count)
            })
            .await
    }

    // ========== FTS5 Search Methods ==========

    /// Search snippets using FTS5 with optional filters.
    ///
    /// Uses the `snippets_fts` virtual table for full-text search on
    /// name, signature, and docs fields.
    pub async fn search_fts(
        &self,
        workspace: &str,
        query: &SymbolQuery,
        limit: i32,
    ) -> Result<Vec<StoredSnippet>> {
        let ws = workspace.to_string();
        let name_pattern = query.name.clone();
        let kind_filter = query.kind.map(|k| k.as_str().to_string());
        let filepath_filter = query.filepath.clone();
        let text_query = query.text.clone();
        let lim = limit;

        self.db
            .query(move |conn| {
                // Build dynamic SQL with parameters
                let use_fts = text_query.is_some();

                let pq = if use_fts {
                    // Join with FTS5 for text search
                    build_fts_query(
                        &ws,
                        &name_pattern,
                        &kind_filter,
                        &filepath_filter,
                        &text_query,
                        lim,
                    )
                } else {
                    // Simple query without FTS
                    build_simple_query(&ws, &name_pattern, &kind_filter, &filepath_filter, lim)
                };

                let mut stmt = conn.prepare(&pq.sql)?;

                // Convert params to rusqlite parameter references
                let param_refs: Vec<&dyn rusqlite::ToSql> = pq
                    .params
                    .iter()
                    .map(|s| s as &dyn rusqlite::ToSql)
                    .collect();

                let rows = stmt.query_map(param_refs.as_slice(), |row| {
                    Ok(StoredSnippet {
                        id: row.get(0)?,
                        workspace: row.get(1)?,
                        filepath: row.get(2)?,
                        name: row.get(3)?,
                        syntax_type: row.get(4)?,
                        start_line: row.get(5)?,
                        end_line: row.get(6)?,
                        signature: row.get(7)?,
                        docs: row.get(8)?,
                        content_hash: row.get(9)?,
                    })
                })?;

                let mut results = Vec::new();
                for row in rows {
                    results.push(row?);
                }
                Ok(results)
            })
            .await
    }

    /// Search snippets by file path using FTS5.
    pub async fn search_by_filepath(
        &self,
        workspace: &str,
        filepath: &str,
        limit: i32,
    ) -> Result<Vec<StoredSnippet>> {
        let query = SymbolQuery::for_file(filepath);
        self.search_fts(workspace, &query, limit).await
    }
}

#[cfg(test)]
#[path = "snippets.test.rs"]
mod tests;
