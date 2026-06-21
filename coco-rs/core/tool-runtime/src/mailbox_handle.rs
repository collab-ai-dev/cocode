//! Teammate mailbox handle — write structured protocol messages into
//! a recipient's inbox without depending on `app/state` from core.
//!
//! **Layering**: definition here; implementation in `coco-state` (which
//! owns the real file I/O in `swarm_mailbox.rs`); consumers in
//! `coco-tools` (ExitPlanModeTool teammate branch, SendMessageTool).
//! Mirrors the existing `SideQuery` / `McpHandle` / `AgentHandle` pattern.

use std::sync::Arc;

/// A minimal protocol-message envelope. The tool-side doesn't need the
/// typed `ProtocolMessage` enum — it hands the already-serialized JSON
/// text to the handle; app/state parses on the other end if needed.
#[derive(Debug, Clone)]
pub struct MailboxEnvelope {
    /// The message body — typically a JSON-serialized protocol message
    /// (e.g. `{"type": "plan_approval_request", ...}`).
    pub text: String,
    /// The sender's agent name. `from` on `TeammateMessage`.
    pub from: String,
    /// ISO-8601 timestamp.
    pub timestamp: String,
}

/// A message in a recipient's inbox, with its index for mark-read ops.
///
/// Returned by `read_unread` so callers can tie messages back to their
/// position for `mark_read_by_index` without re-scanning.
#[derive(Debug, Clone)]
pub struct InboxMessage {
    /// Position in the mailbox file (0-indexed, stable within a read).
    pub index: usize,
    /// Who sent the message.
    pub from: String,
    /// JSON-serialized protocol body (or free text).
    pub text: String,
    /// ISO-8601 timestamp.
    pub timestamp: String,
}

/// Write messages to swarm mailboxes + read the current agent's inbox.
///
/// Implementations wrap `app/state::swarm_mailbox::*`. Absent
/// (`NoOpMailboxHandle`) for non-swarm contexts — calls become no-ops
/// so tools + pollers in single-agent sessions don't crash.
#[async_trait::async_trait]
pub trait MailboxHandle: Send + Sync {
    /// Append `message` to `recipient`'s inbox under the given team.
    async fn write_to_mailbox(
        &self,
        recipient: &str,
        team_name: &str,
        message: MailboxEnvelope,
    ) -> Result<(), coco_error::BoxedError>;

    /// Read unread messages from `agent`'s own inbox. Returns empty on
    /// no-op impls (single-agent sessions).
    async fn read_unread(
        &self,
        agent_name: &str,
        team_name: &str,
    ) -> Result<Vec<InboxMessage>, coco_error::BoxedError>;

    /// Mark a message at `index` as read. Idempotent.
    async fn mark_read(
        &self,
        agent_name: &str,
        team_name: &str,
        index: usize,
    ) -> Result<(), coco_error::BoxedError>;
}

pub type MailboxHandleRef = Arc<dyn MailboxHandle>;

/// No-op implementation for tests and non-swarm sessions.
pub struct NoOpMailboxHandle;

#[async_trait::async_trait]
impl MailboxHandle for NoOpMailboxHandle {
    async fn write_to_mailbox(
        &self,
        _recipient: &str,
        _team_name: &str,
        _message: MailboxEnvelope,
    ) -> Result<(), coco_error::BoxedError> {
        Err(Box::new(coco_error::PlainError::new(
            "mailbox handle not configured — teammate spawn path missing",
            coco_error::StatusCode::Internal,
        )))
    }

    async fn read_unread(
        &self,
        _agent_name: &str,
        _team_name: &str,
    ) -> Result<Vec<InboxMessage>, coco_error::BoxedError> {
        // No-op returns empty — pollers wrapped around this degrade
        // gracefully to "no pending messages".
        Ok(Vec::new())
    }

    async fn mark_read(
        &self,
        _agent_name: &str,
        _team_name: &str,
        _index: usize,
    ) -> Result<(), coco_error::BoxedError> {
        Ok(())
    }
}
