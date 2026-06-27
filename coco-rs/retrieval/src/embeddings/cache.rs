//! Embedding cache to avoid recomputing embeddings for unchanged content.
//!
//! Uses SQLite to persist embeddings keyed by (filepath, content_hash, artifact_id).
//! This allows precise deletion by filepath when files are modified or deleted.
//!
//! Reference: Continue `core/indexing/LanceDbIndex.ts`

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;
use rusqlite::params;

use crate::error::Result;
use crate::error::RetrievalErr;

/// Deduplicated batch result: (hits with embeddings, unique hashes needing embedding).
type DeduplicatedBatchResult = (Vec<(String, String, Vec<f32>)>, Vec<String>);

/// Result of a bulk cache lookup.
///
/// Separates cache hits from misses for efficient processing.
#[derive(Debug, Default)]
pub struct CacheLookupResult {
    /// Found entries: (filepath, content_hash, embedding)
    pub hits: Vec<(String, String, Vec<f32>)>,
    /// Missing entries: (filepath, content_hash)
    pub misses: Vec<(String, String)>,
}

impl CacheLookupResult {
    /// Returns true if all entries were found in cache.
    pub fn all_hits(&self) -> bool {
        self.misses.is_empty()
    }

    /// Returns true if no entries were found in cache.
    pub fn all_misses(&self) -> bool {
        self.hits.is_empty()
    }

    /// Total number of entries queried.
    pub fn total(&self) -> usize {
        self.hits.len() + self.misses.len()
    }

    /// Cache hit ratio (0.0 to 1.0).
    pub fn hit_ratio(&self) -> f32 {
        let total = self.total();
        if total == 0 {
            0.0
        } else {
            self.hits.len() as f32 / total as f32
        }
    }
}

/// Embedding cache backed by SQLite.
///
/// Stores embeddings keyed by (filepath, content_hash), with artifact ID versioning
/// to invalidate cache when the embedding model changes.
///
/// Using filepath as part of the key allows:
/// - Precise deletion when a file is modified or deleted
/// - Simple cache management without reference counting
pub struct EmbeddingCache {
    conn: Mutex<Connection>,
    artifact_id: String,
}

impl EmbeddingCache {
    /// Open or create an embedding cache at the given path.
    ///
    /// # Arguments
    /// * `path` - Path to the SQLite database file
    /// * `artifact_id` - Identifier for the embedding model (e.g., "text-embedding-3-small-v1")
    pub fn open(path: &Path, artifact_id: &str) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| RetrievalErr::SqliteFailed {
            operation: "open embedding cache".to_string(),
            cause: e.to_string(),
        })?;

        // Create embeddings table with (filepath, content_hash, artifact_id) as composite key
        conn.execute(
            "CREATE TABLE IF NOT EXISTS embeddings (
                filepath TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                artifact_id TEXT NOT NULL,
                embedding BLOB NOT NULL,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                PRIMARY KEY (filepath, content_hash, artifact_id)
            )",
            [],
        )
        .map_err(|e| RetrievalErr::SqliteFailed {
            operation: "create embeddings table".to_string(),
            cause: e.to_string(),
        })?;

        // Create index for efficient filepath lookups (for delete_by_filepath)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_embeddings_filepath ON embeddings(filepath)",
            [],
        )
        .map_err(|e| RetrievalErr::SqliteFailed {
            operation: "create filepath index".to_string(),
            cause: e.to_string(),
        })?;

        // Create index for efficient artifact_id lookups
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_embeddings_artifact ON embeddings(artifact_id)",
            [],
        )
        .map_err(|e| RetrievalErr::SqliteFailed {
            operation: "create artifact index".to_string(),
            cause: e.to_string(),
        })?;

        Ok(Self {
            conn: Mutex::new(conn),
            artifact_id: artifact_id.to_string(),
        })
    }

    /// Get a cached embedding for the given filepath and content hash.
    ///
    /// Returns `None` if the embedding is not cached or was created with
    /// a different artifact ID.
    pub fn get(&self, filepath: &str, content_hash: &str) -> Option<Vec<f32>> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        conn.query_row(
            "SELECT embedding FROM embeddings WHERE filepath = ? AND content_hash = ? AND artifact_id = ?",
            params![filepath, content_hash, self.artifact_id],
            |row| {
                let bytes: Vec<u8> = row.get(0)?;
                Ok(bytes_to_f32_vec(&bytes))
            },
        )
        .ok()
    }

    /// Store an embedding in the cache.
    ///
    /// Overwrites any existing entry with the same (filepath, content_hash).
    pub fn put(&self, filepath: &str, content_hash: &str, embedding: &[f32]) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| RetrievalErr::SqliteFailed {
            operation: "lock embedding cache".to_string(),
            cause: "mutex poisoned".to_string(),
        })?;

        let bytes = f32_vec_to_bytes(embedding);
        conn.execute(
            "INSERT OR REPLACE INTO embeddings (filepath, content_hash, artifact_id, embedding) VALUES (?, ?, ?, ?)",
            params![filepath, content_hash, self.artifact_id, bytes],
        )
        .map_err(|e| RetrievalErr::SqliteFailed {
            operation: "insert embedding".to_string(),
            cause: e.to_string(),
        })?;

        Ok(())
    }

    /// Delete all cached embeddings for a filepath.
    ///
    /// Call this when a file is modified or deleted to clean up stale cache entries.
    pub fn delete_by_filepath(&self, filepath: &str) -> Result<i32> {
        let conn = self.conn.lock().map_err(|_| RetrievalErr::SqliteFailed {
            operation: "lock embedding cache".to_string(),
            cause: "mutex poisoned".to_string(),
        })?;

        let count = conn
            .execute(
                "DELETE FROM embeddings WHERE filepath = ?",
                params![filepath],
            )
            .map_err(|e| RetrievalErr::SqliteFailed {
                operation: "delete embeddings by filepath".to_string(),
                cause: e.to_string(),
            })?;

        Ok(count as i32)
    }

    /// Get multiple cached embeddings at once.
    ///
    /// Returns a vector of (filepath, content_hash, embedding) tuples for found entries.
    pub fn get_batch(&self, entries: &[(String, String)]) -> Vec<(String, String, Vec<f32>)> {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let mut results = Vec::new();
        for (filepath, hash) in entries {
            if let Ok(embedding) = conn.query_row(
                "SELECT embedding FROM embeddings WHERE filepath = ? AND content_hash = ? AND artifact_id = ?",
                params![filepath, hash, self.artifact_id],
                |row| {
                    let bytes: Vec<u8> = row.get(0)?;
                    Ok(bytes_to_f32_vec(&bytes))
                },
            ) {
                results.push((filepath.clone(), hash.clone(), embedding));
            }
        }

        results
    }

    /// Store multiple embeddings in the cache.
    ///
    /// Each entry is (filepath, content_hash, embedding).
    pub fn put_batch(&self, entries: &[(String, String, Vec<f32>)]) -> Result<()> {
        let mut conn = self.conn.lock().map_err(|_| RetrievalErr::SqliteFailed {
            operation: "lock embedding cache".to_string(),
            cause: "mutex poisoned".to_string(),
        })?;

        let tx = conn.transaction().map_err(|e| RetrievalErr::SqliteFailed {
            operation: "begin transaction".to_string(),
            cause: e.to_string(),
        })?;

        for (filepath, hash, embedding) in entries {
            let bytes = f32_vec_to_bytes(embedding);
            tx.execute(
                "INSERT OR REPLACE INTO embeddings (filepath, content_hash, artifact_id, embedding) VALUES (?, ?, ?, ?)",
                params![filepath, hash, self.artifact_id, bytes],
            )
            .map_err(|e| RetrievalErr::SqliteFailed {
                operation: "insert embedding batch".to_string(),
                cause: e.to_string(),
            })?;
        }

        tx.commit().map_err(|e| RetrievalErr::SqliteFailed {
            operation: "commit transaction".to_string(),
            cause: e.to_string(),
        })?;

        Ok(())
    }

    /// Remove all embeddings with a different artifact ID.
    ///
    /// Useful for cleaning up stale cache entries after model upgrade.
    pub fn prune_stale(&self) -> Result<i32> {
        let conn = self.conn.lock().map_err(|_| RetrievalErr::SqliteFailed {
            operation: "lock embedding cache".to_string(),
            cause: "mutex poisoned".to_string(),
        })?;

        let count = conn
            .execute(
                "DELETE FROM embeddings WHERE artifact_id != ?",
                params![self.artifact_id],
            )
            .map_err(|e| RetrievalErr::SqliteFailed {
                operation: "prune stale embeddings".to_string(),
                cause: e.to_string(),
            })?;

        Ok(count as i32)
    }

    /// Get the total number of cached embeddings.
    pub fn count(&self) -> Result<i32> {
        let conn = self.conn.lock().map_err(|_| RetrievalErr::SqliteFailed {
            operation: "lock embedding cache".to_string(),
            cause: "mutex poisoned".to_string(),
        })?;

        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM embeddings WHERE artifact_id = ?",
                params![self.artifact_id],
                |row| row.get(0),
            )
            .map_err(|e| RetrievalErr::SqliteFailed {
                operation: "count embeddings".to_string(),
                cause: e.to_string(),
            })?;

        Ok(count)
    }

    /// Get the artifact ID used by this cache.
    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    /// Execute a function with the connection.
    ///
    /// Internal API for cache_ext bulk operations.
    pub(crate) fn with_conn<F, T>(&self, f: F) -> crate::error::Result<T>
    where
        F: FnOnce(&rusqlite::Connection, &str) -> crate::error::Result<T>,
    {
        let conn = self.conn.lock().map_err(|_| RetrievalErr::SqliteFailed {
            operation: "lock embedding cache".to_string(),
            cause: "mutex poisoned".to_string(),
        })?;
        f(&conn, &self.artifact_id)
    }

    /// Bulk lookup using SQL WHERE IN clause.
    ///
    /// More efficient than sequential queries for large batches.
    /// Returns both hits and misses for efficient downstream processing.
    pub fn get_batch_bulk(&self, entries: &[(String, String)]) -> Result<CacheLookupResult> {
        if entries.is_empty() {
            return Ok(CacheLookupResult::default());
        }

        let entries_clone: Vec<(String, String)> = entries.to_vec();

        self.with_conn(|conn, artifact_id| {
            let mut conditions = Vec::with_capacity(entries_clone.len());
            let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

            for (filepath, hash) in &entries_clone {
                conditions.push("(filepath = ? AND content_hash = ?)");
                params_vec.push(Box::new(filepath.clone()));
                params_vec.push(Box::new(hash.clone()));
            }

            let query = format!(
                "SELECT filepath, content_hash, embedding FROM embeddings WHERE artifact_id = ? AND ({})",
                conditions.join(" OR ")
            );

            let mut all_params: Vec<Box<dyn rusqlite::ToSql>> =
                vec![Box::new(artifact_id.to_string())];
            all_params.extend(params_vec);

            let params_refs: Vec<&dyn rusqlite::ToSql> =
                all_params.iter().map(|p| p.as_ref()).collect();

            let mut stmt = conn
                .prepare(&query)
                .map_err(|e| RetrievalErr::SqliteFailed {
                    operation: "prepare bulk lookup".to_string(),
                    cause: e.to_string(),
                })?;

            let rows = stmt
                .query_map(params_refs.as_slice(), |row| {
                    let filepath: String = row.get(0)?;
                    let content_hash: String = row.get(1)?;
                    let bytes: Vec<u8> = row.get(2)?;
                    Ok((filepath, content_hash, bytes_to_f32_vec(&bytes)))
                })
                .map_err(|e| RetrievalErr::SqliteFailed {
                    operation: "execute bulk lookup".to_string(),
                    cause: e.to_string(),
                })?;

            let mut hits = Vec::new();
            let mut found_keys: HashSet<(String, String)> = HashSet::new();

            for (filepath, hash, embedding) in rows.flatten() {
                found_keys.insert((filepath.clone(), hash.clone()));
                hits.push((filepath, hash, embedding));
            }

            let misses: Vec<(String, String)> = entries_clone
                .iter()
                .filter(|(f, h)| !found_keys.contains(&(f.clone(), h.clone())))
                .cloned()
                .collect();

            Ok(CacheLookupResult { hits, misses })
        })
    }

    /// Lookup with deduplication by content hash.
    ///
    /// When multiple files have identical content, only one embedding needs to
    /// be computed. Returns unique content hashes that need embedding.
    pub fn get_batch_deduplicated(
        &self,
        entries: &[(String, String)],
    ) -> Result<DeduplicatedBatchResult> {
        let result = self.get_batch_bulk(entries)?;

        let mut unique_hashes: HashSet<String> = HashSet::new();
        for (_, hash) in &result.misses {
            unique_hashes.insert(hash.clone());
        }

        Ok((result.hits, unique_hashes.into_iter().collect()))
    }
}

/// Convert a byte slice to a Vec<f32>.
pub(crate) fn bytes_to_f32_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Convert a Vec<f32> to bytes.
fn f32_vec_to_bytes(floats: &[f32]) -> Vec<u8> {
    floats.iter().flat_map(|f| f.to_le_bytes()).collect()
}

#[cfg(test)]
#[path = "cache.test.rs"]
mod tests;
