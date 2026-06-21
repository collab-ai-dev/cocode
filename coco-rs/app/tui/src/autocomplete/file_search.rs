//! File search autocomplete (@path mentions).
//!
//! Debounced async search backed by `coco_file_search::FileIndex` —
//! nucleo fuzzy matching + git-aware discovery + 60s cache TTL.

use std::time::Duration;

use coco_file_search::FileIndex;
use coco_file_search::SharedFileIndex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::completion::CompletionRequestKey;
use crate::widgets::suggestion_popup::SuggestionItem;
use crate::widgets::suggestion_popup::SuggestionMeta;

/// Debounce delay for file search.
const DEBOUNCE: Duration = Duration::from_millis(50);

/// Maximum suggestions returned.
const MAX_SUGGESTIONS: i32 = 15;

/// Events from file search to TUI.
#[derive(Debug, Clone)]
pub enum FileSearchEvent {
    /// Search results ready.
    SearchResult {
        key: CompletionRequestKey,
        suggestions: Vec<SuggestionItem>,
    },
}

/// Manages debounced file search backed by a shared `FileIndex`.
pub struct FileSearchManager {
    index: SharedFileIndex,
    pending: Option<JoinHandle<()>>,
    event_tx: mpsc::Sender<FileSearchEvent>,
}

impl FileSearchManager {
    /// Create a new file search manager backed by `index`.
    pub fn new(index: SharedFileIndex, event_tx: mpsc::Sender<FileSearchEvent>) -> Self {
        Self {
            index,
            pending: None,
            event_tx,
        }
    }

    /// Schedule a debounced search.
    pub fn search(&mut self, key: CompletionRequestKey) {
        if let Some(handle) = self.pending.take() {
            handle.abort();
        }

        let index = self.index.clone();
        let tx = self.event_tx.clone();

        self.pending = Some(tokio::spawn(async move {
            tokio::time::sleep(DEBOUNCE).await;
            let query = key.query.clone();

            FileIndex::refresh_if_stale(&index).await;

            let suggestions = {
                let guard = index.read().await;
                guard
                    .get_suggestions(&query, MAX_SUGGESTIONS)
                    .into_iter()
                    .map(|s| SuggestionItem {
                        label: s.path,
                        description: None,
                        metadata: Some(SuggestionMeta::Path {
                            is_directory: s.is_directory,
                        }),
                    })
                    .collect::<Vec<_>>()
            };

            let _ = tx
                .send(FileSearchEvent::SearchResult { key, suggestions })
                .await;
        }));
    }

    /// Cancel any pending search.
    pub fn cancel(&mut self) {
        if let Some(handle) = self.pending.take() {
            handle.abort();
        }
    }

    /// Update the underlying file index (e.g. on cwd change).
    pub fn set_index(&mut self, index: SharedFileIndex) {
        self.index = index;
    }
}

/// Create the file search channel.
pub fn create_file_search_channel() -> (
    mpsc::Sender<FileSearchEvent>,
    mpsc::Receiver<FileSearchEvent>,
) {
    mpsc::channel(16)
}
