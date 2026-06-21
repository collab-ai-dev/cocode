//! Per-recipient pending-message store.
//!
//! Per-recipient FIFO of strings queued by `SendMessageTool` for a recipient agent
//! that's currently running. The recipient's next turn drains the
//! queue via the `agent_pending_messages` system-reminder, which
//! renders each item as a `queued_command` attachment with `origin:
//! coordinator` framing.
//!
//! ## Why a dedicated trait
//!
//! - `MailboxHandle` is file-backed teammate IPC across processes.
//!   Pending messages are an in-memory same-process queue with
//!   different lifecycle semantics (drained-on-read, no acknowledgment).
//! - Putting the data on `TaskStateBase` would require every consumer
//!   that reads tasks to thread it through, and serialise the queue in
//!   wire transcripts where it doesn't belong.
//!
//! - `tasks/LocalAgentTask/LocalAgentTask.tsx:136` —
//!   `pendingMessages: string[]` field on `LocalAgentTaskState`.
//! - `tasks/LocalAgentTask/LocalAgentTask.tsx:162-167` —
//!   `queuePendingMessage(taskId, msg)` appends.
//! - `tasks/LocalAgentTask/LocalAgentTask.tsx:181-192` —
//!   `drainPendingMessages(taskId)` returns + clears.
//! - `utils/attachments.ts:1085-1101` —
//!   `getAgentPendingMessageAttachments` drains and maps to
//!   `queued_command` attachments.
//! - `utils/task/framework.ts:82-95` — on `resumeAgentBackground`
//!   re-register, `pendingMessages` is carried forward from the
//!   existing task.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::RwLock;

/// One queued message. `from` carries the sender's agent name (for the
/// `agent_pending_messages` reminder template, which shows the sender).
#[derive(Debug, Clone)]
pub struct PendingMessage {
    /// Sender agent name (or `"main"` for the user's main thread).
    pub from: String,
    /// The text the sender supplied. Free-form — typically a plain
    /// string; structured protocol messages (shutdown_request etc.)
    /// don't queue here, they route through `MailboxHandle`.
    pub text: String,
}

/// Per-recipient FIFO queue. Implementations live in app/cli alongside
/// `TaskRuntime`; `NoOpPendingMessageStore` returns empty for headless
/// sessions and tests without swarm wiring.
#[async_trait::async_trait]
pub trait PendingMessageStore: Send + Sync {
    /// Append `message` to `recipient`'s FIFO. No-op when the store
    /// isn't wired (e.g. headless SDK).
    async fn push(&self, recipient_agent_id: &str, message: PendingMessage);

    /// Drain (return + clear) every pending message for `recipient_agent_id`.
    /// Returns an empty Vec when the recipient has nothing queued or
    /// the store isn't wired. Drain-on-read: once the reminder generator
    /// has them, they are gone from the queue.
    async fn drain(&self, recipient_agent_id: &str) -> Vec<PendingMessage>;

    /// Peek the queue without draining. Used by the system-reminder
    /// generator on cache-warm paths where the drain happens elsewhere
    /// (e.g. when the auto-resume flow has already consumed the
    /// queue). Default returns drain-equivalent.
    async fn peek(&self, recipient_agent_id: &str) -> Vec<PendingMessage> {
        self.drain(recipient_agent_id).await
    }
}

pub type PendingMessageStoreRef = Arc<dyn PendingMessageStore>;

/// No-op implementation. Calls to `push` succeed silently; `drain`
/// returns empty. Used in headless / single-agent contexts where no
/// teammate routing exists.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoOpPendingMessageStore;

#[async_trait::async_trait]
impl PendingMessageStore for NoOpPendingMessageStore {
    async fn push(&self, _recipient_agent_id: &str, _message: PendingMessage) {}
    async fn drain(&self, _recipient_agent_id: &str) -> Vec<PendingMessage> {
        Vec::new()
    }
}

/// In-memory FIFO implementation. Cheap to clone (single `Arc<RwLock<...>>`);
/// production wires one instance per session and hands `Arc<Self>` to both
/// the tool layer (`ToolUseContext.pending_messages`) and the reminder
/// adapter (`SwarmAdapter`).
#[derive(Debug, Default)]
pub struct InMemoryPendingMessageStore {
    queues: RwLock<HashMap<String, VecDeque<PendingMessage>>>,
}

impl InMemoryPendingMessageStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl PendingMessageStore for InMemoryPendingMessageStore {
    async fn push(&self, recipient_agent_id: &str, message: PendingMessage) {
        self.queues
            .write()
            .await
            .entry(recipient_agent_id.to_string())
            .or_default()
            .push_back(message);
    }

    async fn drain(&self, recipient_agent_id: &str) -> Vec<PendingMessage> {
        let mut guard = self.queues.write().await;
        match guard.get_mut(recipient_agent_id) {
            Some(q) if !q.is_empty() => q.drain(..).collect(),
            _ => Vec::new(),
        }
    }

    async fn peek(&self, recipient_agent_id: &str) -> Vec<PendingMessage> {
        self.queues
            .read()
            .await
            .get(recipient_agent_id)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "pending_messages.test.rs"]
mod tests;
