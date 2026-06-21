//! Plan-approval mailbox waiter for the in-process teammate loop.
//!
//! Extracted from `runner_loop.rs` (P1 split). Sibling
//! `wait_for_next_prompt_or_shutdown` stays in the main file because it
//! shares the loop's `POLL_INTERVAL_MS` and the priority-comment
//! ordering reads more naturally next to its mailbox match arms.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::mailbox;
use crate::types::TeammateIdentity;

/// Poll interval for inbox scanning (ms). Mirrors the constant in
/// `runner_loop.rs`.
const POLL_INTERVAL_MS: u64 = 500;

/// Wait until the leader sends a [`mailbox::ProtocolMessage::PlanApprovalResponse`]
/// matching `request_id`. Polls the teammate's inbox at
/// [`POLL_INTERVAL_MS`] and respects the cancellation flag.
pub async fn wait_for_plan_approval(
    identity: &TeammateIdentity,
    cancelled: &AtomicBool,
    request_id: &str,
) -> Option<(bool, Option<String>)> {
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let messages =
            mailbox::read_mailbox(&identity.agent_name, &identity.team_name).unwrap_or_default();
        for (i, msg) in messages.iter().enumerate() {
            if msg.read {
                continue;
            }
            if !mailbox::is_structured_protocol_message(&msg.text) {
                continue;
            }
            if let Some(mailbox::ProtocolMessage::PlanApprovalResponse {
                request_id: rid,
                approved,
                feedback,
                ..
            }) = mailbox::parse_protocol_message(&msg.text)
                && rid == request_id
            {
                let _ = mailbox::mark_message_as_read_by_index(
                    &identity.agent_name,
                    &identity.team_name,
                    i,
                );
                return Some((approved, feedback));
            }
        }
        tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
}
