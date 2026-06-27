//! Symbol search autocomplete (@#symbol mentions).
//!
//! Debounced async search against LSP symbol index.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::completion::CompletionRequestKey;
use crate::widgets::suggestion_popup::SuggestionItem;

/// Debounce delay for symbol search.
const DEBOUNCE: Duration = Duration::from_millis(150);

pub trait SymbolCompletionSource: Send + Sync {
    fn search(&self, query: &str) -> Vec<SuggestionItem>;
}

#[derive(Debug, Default)]
pub struct NoopSymbolCompletionSource;

impl SymbolCompletionSource for NoopSymbolCompletionSource {
    fn search(&self, _query: &str) -> Vec<SuggestionItem> {
        Vec::new()
    }
}

/// Events from symbol search to TUI.
#[derive(Debug, Clone)]
pub enum SymbolSearchEvent {
    /// Search results ready.
    SearchResult {
        key: CompletionRequestKey,
        suggestions: Vec<SuggestionItem>,
    },
}

/// Manages debounced symbol search via LSP.
pub struct SymbolSearchManager {
    pending: Option<JoinHandle<()>>,
    event_tx: mpsc::Sender<SymbolSearchEvent>,
    source: Arc<dyn SymbolCompletionSource>,
}

impl SymbolSearchManager {
    /// Create a new symbol search manager.
    pub fn new(event_tx: mpsc::Sender<SymbolSearchEvent>) -> Self {
        Self::with_source(event_tx, Arc::new(NoopSymbolCompletionSource))
    }

    pub fn with_source(
        event_tx: mpsc::Sender<SymbolSearchEvent>,
        source: Arc<dyn SymbolCompletionSource>,
    ) -> Self {
        Self {
            pending: None,
            event_tx,
            source,
        }
    }

    /// Schedule a debounced search.
    pub fn search(&mut self, key: CompletionRequestKey) {
        if let Some(handle) = self.pending.take() {
            handle.abort();
        }

        let tx = self.event_tx.clone();
        let source = Arc::clone(&self.source);

        self.pending = Some(tokio::spawn(async move {
            tokio::time::sleep(DEBOUNCE).await;
            let query = key.query.clone();
            let suggestions = source.search(&query);

            let _ = tx
                .send(SymbolSearchEvent::SearchResult { key, suggestions })
                .await;
        }));
    }

    /// Cancel any pending search.
    pub fn cancel(&mut self) {
        if let Some(handle) = self.pending.take() {
            handle.abort();
        }
    }
}

/// Create the symbol search channel.
pub fn create_symbol_search_channel() -> (
    mpsc::Sender<SymbolSearchEvent>,
    mpsc::Receiver<SymbolSearchEvent>,
) {
    mpsc::channel(16)
}

#[cfg(test)]
#[path = "symbol_search.test.rs"]
mod tests;
