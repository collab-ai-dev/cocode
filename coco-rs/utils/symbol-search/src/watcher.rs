//! File watcher for incremental symbol index updates.

use std::path::PathBuf;

use crate::languages::SymbolLanguage;

/// Event emitted when symbol-relevant files change.
#[derive(Debug, Clone)]
pub struct SymbolFileChanged {
    /// Paths that changed (relative to root).
    pub paths: Vec<PathBuf>,
}

/// Classify a filesystem event — only include files with supported extensions.
pub fn classify_event(event: &notify::Event) -> Option<SymbolFileChanged> {
    let paths: Vec<PathBuf> = event
        .paths
        .iter()
        .filter(|p| SymbolLanguage::from_path(p).is_some())
        .cloned()
        .collect();

    if paths.is_empty() {
        None
    } else {
        Some(SymbolFileChanged { paths })
    }
}

/// Merge two change events by combining their paths.
pub fn merge_events(mut acc: SymbolFileChanged, new: SymbolFileChanged) -> SymbolFileChanged {
    acc.paths.extend(new.paths);
    acc
}
