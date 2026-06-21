//! File-based mailbox system for inter-teammate messaging.
//!
//! Inbox layout: `~/.coco/teams/{team_name}/inboxes/{agent_name}.json`
//! — a JSON array of [`TeammateMessage`] read-modify-written under an
//! advisory `fs2` lock. Mailboxes also carry structured protocol
//! envelopes (idle, permission, plan approval, shutdown, mode change)
//! discriminated by a JSON `type` tag.
//!
//! # Module split
//!
//! | Submodule | Responsibility |
//! |-----------|----------------|
//! | [`io`] | Inbox path, JSON read / write, mark-as-read, format helpers, [`TeammateMessage`] |
//! | [`lock`] | `fs2` advisory locking with retry + jitter (private to mailbox) |
//! | [`protocol`] | [`ProtocolMessage`] enum, creators, type checkers, permission-sync directories |
//!
//! Top-level re-exports keep the historic API stable: callers use
//! `crate::mailbox::write_to_mailbox`, `crate::mailbox::ProtocolMessage`,
//! etc. without caring about which submodule actually owns the symbol.

pub mod io;
pub(crate) mod lock;
pub mod protocol;

// ── Re-exports — keep the legacy `coordinator::mailbox::*` flat API ──

pub use io::{
    TeammateMessage, clear_mailbox, format_teammate_messages, inbox_path,
    mark_message_as_read_by_index, mark_messages_as_read, mark_messages_as_read_by_predicate,
    read_mailbox, read_unread_messages, write_to_mailbox,
};
pub use protocol::{
    ProtocolMessage, check_message_type, cleanup_old_resolutions, create_idle_notification,
    create_mode_set_request, create_permission_request_message, create_permission_response_message,
    create_plan_approval_request_message, create_shutdown_approved_message,
    create_shutdown_rejected_message, ensure_permission_dirs, is_structured_protocol_message,
    parse_protocol_message, pending_permissions_dir, permissions_dir, poll_for_response,
    read_pending_permissions, read_resolved_permission, remove_worker_response, resolve_permission,
    resolved_permissions_dir, send_permission_request_via_mailbox,
    send_permission_response_via_mailbox, send_sandbox_permission_request_via_mailbox,
    send_sandbox_permission_response_via_mailbox, send_shutdown_request, write_pending_permission,
};

// ── Leader/Worker Identity Helpers ──

/// Check if the current agent is the team leader.
pub fn is_team_leader(team_name: &str) -> bool {
    let agent_name = crate::identity::get_agent_name();
    agent_name.as_deref() == Some(crate::constants::TEAM_LEAD_NAME)
        || !crate::identity::is_teammate()
        || crate::team_file::read_team_file(team_name)
            .ok()
            .flatten()
            .is_some_and(|tf| {
                crate::identity::get_agent_id().is_some_and(|id| id == tf.lead_agent_id)
            })
}

/// Check if the current agent is a swarm worker.
pub fn is_swarm_worker() -> bool {
    crate::identity::is_teammate()
}

/// Get the leader's agent name from the team file.
pub fn get_leader_name(_team_name: &str) -> String {
    crate::constants::TEAM_LEAD_NAME.to_string()
}

// ── Protocol-message helpers ──

/// Generate a deterministic-by-agent-identity-but-unique-per-call
/// request ID for plan_approval.
pub fn generate_plan_approval_request_id(agent_name: &str, team_name: &str) -> String {
    // Short random suffix — collisions within a session are astronomically
    // unlikely and the correlation is handled by the leader matching on
    // the full string anyway.
    let rand: String = uuid::Uuid::new_v4().simple().to_string();
    let rand8: String = rand.chars().take(8).collect();
    format!("plan_approval-{agent_name}-{team_name}-{rand8}")
}

// ── MailboxHandle impl for ToolUseContext plumbing (`coco-tool-runtime` trait) ──

/// Concrete `MailboxHandle` implementation that writes via
/// [`write_to_mailbox`]. Engines and spawn paths install one of these
/// on the teammate's `ToolUseContext` so ExitPlanMode + SendMessage can
/// reach the leader's inbox without crossing layer boundaries directly.
#[derive(Debug, Default)]
pub struct SwarmMailboxHandle;

fn boxed_coordinator_err(e: crate::CoordinatorError) -> coco_error::BoxedError {
    Box::new(e)
}

#[async_trait::async_trait]
impl coco_tool_runtime::MailboxHandle for SwarmMailboxHandle {
    async fn write_to_mailbox(
        &self,
        recipient: &str,
        team_name: &str,
        message: coco_tool_runtime::MailboxEnvelope,
    ) -> Result<(), coco_error::BoxedError> {
        let msg = TeammateMessage {
            from: message.from,
            text: message.text,
            timestamp: message.timestamp,
            read: false,
            color: None,
            summary: None,
        };
        write_to_mailbox(recipient, msg, team_name).map_err(boxed_coordinator_err)
    }

    async fn read_unread(
        &self,
        agent_name: &str,
        team_name: &str,
    ) -> Result<Vec<coco_tool_runtime::InboxMessage>, coco_error::BoxedError> {
        // We need indices from the FULL mailbox (to support
        // `mark_read(index)`), so read the full list and filter to
        // unread in-place.
        let all = read_mailbox(agent_name, team_name).unwrap_or_default();
        Ok(all
            .into_iter()
            .enumerate()
            .filter(|(_, m)| !m.read)
            .map(|(index, m)| coco_tool_runtime::InboxMessage {
                index,
                from: m.from,
                text: m.text,
                timestamp: m.timestamp,
            })
            .collect())
    }

    async fn mark_read(
        &self,
        agent_name: &str,
        team_name: &str,
        index: usize,
    ) -> Result<(), coco_error::BoxedError> {
        mark_message_as_read_by_index(agent_name, team_name, index).map_err(boxed_coordinator_err)
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
