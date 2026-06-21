//! Async explicit path completion provider.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::completion::CompletionRequestKey;
use crate::completion::SuggestionKind;
use crate::widgets::suggestion_popup::SuggestionItem;
use crate::widgets::suggestion_popup::SuggestionMeta;

const TIMEOUT: Duration = Duration::from_secs(1);
const MAX_SUGGESTIONS: usize = 15;
const MAX_BLOCKING_SCANS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMode {
    FilesAndDirectories,
    DirectoriesOnly,
}

#[derive(Debug, Clone)]
pub enum PathCompletionEvent {
    SearchResult {
        key: CompletionRequestKey,
        suggestions: Vec<SuggestionItem>,
    },
}

pub struct PathCompletionManager {
    pending: Option<JoinHandle<()>>,
    event_tx: mpsc::Sender<PathCompletionEvent>,
}

impl PathCompletionManager {
    pub fn new(event_tx: mpsc::Sender<PathCompletionEvent>) -> Self {
        Self {
            pending: None,
            event_tx,
        }
    }

    pub fn search(&mut self, key: CompletionRequestKey) {
        if let Some(handle) = self.pending.take() {
            handle.abort();
        }

        let mode = match key.kind {
            SuggestionKind::Directory => PathMode::DirectoriesOnly,
            SuggestionKind::Path => PathMode::FilesAndDirectories,
            _ => return,
        };
        let tx = self.event_tx.clone();
        self.pending = Some(tokio::spawn(async move {
            let blocking_query = key.query.clone();
            let permit =
                match tokio::time::timeout(TIMEOUT, path_scan_semaphore().acquire_owned()).await {
                    Ok(Ok(permit)) => permit,
                    Ok(Err(_)) | Err(_) => {
                        let _ = tx
                            .send(PathCompletionEvent::SearchResult {
                                key,
                                suggestions: Vec::new(),
                            })
                            .await;
                        return;
                    }
                };
            let task = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                path_items(&blocking_query, mode)
            });
            let suggestions = match tokio::time::timeout(TIMEOUT, task).await {
                Ok(Ok(items)) => items,
                Ok(Err(err)) => {
                    tracing::warn!(
                        target: "coco_tui::completion",
                        error = %err,
                        "path completion task failed",
                    );
                    Vec::new()
                }
                Err(_) => Vec::new(),
            };
            let _ = tx
                .send(PathCompletionEvent::SearchResult { key, suggestions })
                .await;
        }));
    }

    pub fn cancel(&mut self) {
        if let Some(handle) = self.pending.take() {
            handle.abort();
        }
    }
}

pub fn create_path_completion_channel() -> (
    mpsc::Sender<PathCompletionEvent>,
    mpsc::Receiver<PathCompletionEvent>,
) {
    mpsc::channel(16)
}

fn path_scan_semaphore() -> Arc<Semaphore> {
    static SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEMAPHORE
        .get_or_init(|| Arc::new(Semaphore::new(MAX_BLOCKING_SCANS)))
        .clone()
}

fn path_items(query: &str, mode: PathMode) -> Vec<SuggestionItem> {
    let Some(parsed) = ParsedPathQuery::parse(query) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&parsed.fs_dir) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !name.starts_with(parsed.partial) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let is_directory = file_type.is_dir();
        if matches!(mode, PathMode::DirectoriesOnly) && !is_directory {
            continue;
        }
        items.push(SuggestionItem {
            label: format!("{}{}", parsed.display_dir, name),
            description: Some(if is_directory { "directory" } else { "file" }.to_string()),
            metadata: Some(SuggestionMeta::Path { is_directory }),
        });
    }
    items.sort_by(|a, b| {
        let a_dir = matches!(
            a.metadata.as_ref(),
            Some(SuggestionMeta::Path { is_directory: true })
        );
        let b_dir = matches!(
            b.metadata.as_ref(),
            Some(SuggestionMeta::Path { is_directory: true })
        );
        b_dir.cmp(&a_dir).then_with(|| a.label.cmp(&b.label))
    });
    items.truncate(MAX_SUGGESTIONS);
    items
}

struct ParsedPathQuery<'a> {
    fs_dir: PathBuf,
    display_dir: String,
    partial: &'a str,
}

impl<'a> ParsedPathQuery<'a> {
    fn parse(query: &'a str) -> Option<Self> {
        let query = query.strip_prefix('"').unwrap_or(query);
        let slash = query.rfind('/')?;
        let (display_dir, partial) = query.split_at(slash + 1);
        let fs_dir = if display_dir == "~/" {
            std::env::var_os("HOME").map(PathBuf::from)?
        } else if let Some(rest) = display_dir.strip_prefix("~/") {
            std::env::var_os("HOME").map(PathBuf::from)?.join(rest)
        } else {
            PathBuf::from(display_dir)
        };
        Some(Self {
            fs_dir,
            display_dir: display_dir.to_string(),
            partial,
        })
    }
}

#[cfg(test)]
#[path = "path_provider.test.rs"]
mod tests;
