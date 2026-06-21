//! Per-agent ephemeral checklist backing the legacy `TodoWrite` tool
//! (V1). Deliberately minimal — stored only in memory, never on disk.
//! Keyed by `agent_id ?? session_id`.

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;

/// A single todo item: `{ content: min(1), status: 'pending'|'in_progress'|'completed', activeForm: min(1) }`.
///
/// No `id` field — uses positional identity and replace-all semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    #[serde(rename = "activeForm")]
    pub active_form: String,
}

impl TodoItem {
    pub fn is_valid_status(s: &str) -> bool {
        matches!(s, "pending" | "in_progress" | "completed")
    }
}

/// In-memory per-agent todo store. Each agent (or the main session)
/// gets its own `Vec<TodoItem>` list, keyed by `agent_id ?? session_id`.
///
/// Single `Mutex` is fine — reads and writes are infrequent (one per
/// TodoWrite call) and there's no blocking I/O under the lock.
#[derive(Default)]
pub struct TodoStore {
    inner: Mutex<HashMap<String, Vec<TodoItem>>>,
}

impl TodoStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read the list for `key` (empty if unset).
    pub fn read(&self, key: &str) -> Vec<TodoItem> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    /// Replace the list for `key`.
    pub fn write(&self, key: &str, items: Vec<TodoItem>) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if items.is_empty() {
            guard.remove(key);
        } else {
            guard.insert(key.to_string(), items);
        }
    }

    /// Clear all entries (tests only).
    pub fn clear_all(&self) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

#[cfg(test)]
#[path = "todos.test.rs"]
mod tests;
