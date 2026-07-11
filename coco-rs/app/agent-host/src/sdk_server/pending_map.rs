//! Generic correlation map for server-issued pending requests.
//!
//! Used by `SdkServerState` to track outbound `ServerRequest`s whose
//! response the agent is awaiting. Each flavor (approval, user-input,
//! hook-callback, mcp-route, elicitation) reuses this same type parameterized
//! by its response payload.
//!
//! Without this abstraction, each flavor grew its own duplicated
//! `HashMap<String, oneshot::Sender<T>>` field, its own `register_*` method,
//! and its own `handle_*_resolve` function — the only difference being the
//! response type. `handle_cancel_request` also historically only cleaned up
//! two of the five maps; iterating `PendingMap::remove` across all five is
//! now trivial and uniform.

use std::collections::HashMap;

use tokio::sync::Mutex;
use tokio::sync::oneshot;

/// Correlation map from `request_id` → oneshot sender awaiting a typed
/// response payload.
pub struct PendingMap<T> {
    inner: Mutex<HashMap<String, oneshot::Sender<T>>>,
}

impl<T> PendingMap<T> {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Register an expected response. The caller must next send the
    /// corresponding outbound `ServerRequest`. The returned receiver wakes
    /// when `resolve` fires for the same `id`.
    pub async fn register(&self, id: String) -> oneshot::Receiver<T> {
        let (tx, rx) = oneshot::channel();
        self.inner.lock().await.insert(id, tx);
        rx
    }

    /// Remove the entry and attempt to deliver the payload to its receiver.
    pub async fn resolve(&self, id: &str, payload: T) -> ResolveOutcome {
        let sender = self.inner.lock().await.remove(id);
        match sender {
            None => ResolveOutcome::NotFound,
            Some(tx) => {
                if tx.send(payload).is_ok() {
                    ResolveOutcome::Delivered
                } else {
                    ResolveOutcome::ReceiverDropped
                }
            }
        }
    }

    /// Remove the entry without delivering a payload (cancellation path).
    /// Returns `true` if an entry was removed.
    pub async fn remove(&self, id: &str) -> bool {
        self.inner.lock().await.remove(id).is_some()
    }
}

impl<T> Default for PendingMap<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of `PendingMap::resolve` — distinguishes the three outcomes so
/// handlers can respond with the right JSON-RPC error code (or ok) and the
/// right log level.
#[derive(Debug, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// Entry was found and the payload was handed to the receiver.
    Delivered,
    /// Entry was found but the receiver has been dropped (e.g. the agent's
    /// awaiter was cancelled before the client replied). The client's reply
    /// is still acknowledged — we just log the race.
    ReceiverDropped,
    /// No pending entry matched the id. Typical causes: duplicate resolve,
    /// stale reply after turn cancellation, protocol confusion.
    NotFound,
}

#[cfg(test)]
#[path = "pending_map.test.rs"]
mod tests;
