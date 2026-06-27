//! In-memory symbol index with fuzzy search.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use coco_file_ignore::IgnoreConfig;
use coco_file_ignore::IgnoreService;
use coco_utils_common::fuzzy_match;

use crate::SymbolKind;
use crate::SymbolSearchResult;
use crate::error::SymbolSearchError;
use crate::extractor::SymbolExtractor;
use crate::languages::SymbolLanguage;

/// Entry stored in the index for each symbol.
#[derive(Debug, Clone)]
struct SymbolEntry {
    /// Symbol name (original case).
    name: String,
    /// Lowercased name for matching.
    name_lower: String,
    /// Symbol kind.
    kind: SymbolKind,
    /// File path relative to root.
    file_path: String,
    /// Line number (1-indexed).
    line: i32,
}

/// In-memory symbol index.
pub struct SymbolIndex {
    /// All symbol entries, grouped by file.
    entries_by_file: HashMap<String, Vec<SymbolEntry>>,
    /// Flat list of all entries (for search).
    all_entries: Vec<SymbolEntry>,
}

impl SymbolIndex {
    /// Build index by scanning all supported files under root.
    ///
    /// This is CPU-bound; call from `spawn_blocking`.
    pub fn build(root: &Path) -> Result<Self, SymbolSearchError> {
        let mut extractor = SymbolExtractor::new();
        let mut entries_by_file: HashMap<String, Vec<SymbolEntry>> = HashMap::new();

        let config = IgnoreConfig::default()
            .with_hidden(false)
            .with_follow_links(true);
        let walker = IgnoreService::new(config).create_walk_builder(root);

        for entry in walker.build() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                continue;
            }

            let path = entry.path();
            if SymbolLanguage::from_path(path).is_none() {
                continue;
            }

            let rel_path = match path.strip_prefix(root) {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(_) => continue,
            };

            match extractor.extract_file(path) {
                Ok(tags) => {
                    let file_entries: Vec<SymbolEntry> = tags
                        .into_iter()
                        .filter(|t| t.is_definition)
                        .map(|t| SymbolEntry {
                            name_lower: t.name.to_lowercase(),
                            name: t.name,
                            kind: t.kind,
                            file_path: rel_path.clone(),
                            line: t.line,
                        })
                        .collect();
                    if !file_entries.is_empty() {
                        entries_by_file.insert(rel_path, file_entries);
                    }
                }
                Err(e) => {
                    tracing::trace!(path = %path.display(), error = %e, "Failed to extract tags");
                }
            }
        }

        let all_entries: Vec<SymbolEntry> = entries_by_file.values().flatten().cloned().collect();

        tracing::info!(
            files = entries_by_file.len(),
            symbols = all_entries.len(),
            "Symbol index built"
        );

        Ok(Self {
            entries_by_file,
            all_entries,
        })
    }

    /// Re-extract tags for changed files, update index in-place.
    pub fn update_files(
        &mut self,
        root: &Path,
        changed: &[PathBuf],
    ) -> Result<(), SymbolSearchError> {
        let mut extractor = SymbolExtractor::new();

        for path in changed {
            let full_path = if path.is_absolute() {
                path.clone()
            } else {
                root.join(path)
            };

            let rel_path = match full_path.strip_prefix(root) {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(_) => continue,
            };

            if !full_path.exists() {
                self.entries_by_file.remove(&rel_path);
                continue;
            }

            if SymbolLanguage::from_path(&full_path).is_none() {
                continue;
            }

            match extractor.extract_file(&full_path) {
                Ok(tags) => {
                    let file_entries: Vec<SymbolEntry> = tags
                        .into_iter()
                        .filter(|t| t.is_definition)
                        .map(|t| SymbolEntry {
                            name_lower: t.name.to_lowercase(),
                            name: t.name,
                            kind: t.kind,
                            file_path: rel_path.clone(),
                            line: t.line,
                        })
                        .collect();
                    if file_entries.is_empty() {
                        self.entries_by_file.remove(&rel_path);
                    } else {
                        self.entries_by_file.insert(rel_path, file_entries);
                    }
                }
                Err(_) => {
                    self.entries_by_file.remove(&rel_path);
                }
            }
        }

        // Rebuild flat list
        self.all_entries = self.entries_by_file.values().flatten().cloned().collect();
        Ok(())
    }

    /// Remove entries for a deleted file.
    pub fn remove_file(&mut self, file_path: &str) {
        if self.entries_by_file.remove(file_path).is_some() {
            self.all_entries = self.entries_by_file.values().flatten().cloned().collect();
        }
    }

    /// Fuzzy search symbol names. Returns top `limit` results.
    pub fn search(&self, query: &str, limit: i32) -> Vec<SymbolSearchResult> {
        if query.is_empty() {
            return Vec::new();
        }

        let query_lower = query.to_lowercase();

        let mut scored: Vec<(i32, Vec<usize>, &SymbolEntry)> = self
            .all_entries
            .iter()
            .filter_map(|entry| {
                fuzzy_match(&entry.name_lower, &query_lower)
                    .map(|(indices, score)| (score, indices, entry))
            })
            .collect();

        // Sort by score ascending (lower = better), then alphabetically
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.name.cmp(&b.2.name)));

        scored
            .into_iter()
            .take(limit as usize)
            .map(|(score, match_indices, entry)| SymbolSearchResult {
                name: entry.name.clone(),
                kind: entry.kind,
                file_path: entry.file_path.clone(),
                line: entry.line,
                score,
                match_indices,
            })
            .collect()
    }

    /// Get total number of indexed symbols.
    pub fn len(&self) -> i32 {
        self.all_entries.len() as i32
    }

    /// Check if the index is empty.
    pub fn is_empty(&self) -> bool {
        self.all_entries.is_empty()
    }
}

#[cfg(test)]
#[path = "index.test.rs"]
mod tests;
