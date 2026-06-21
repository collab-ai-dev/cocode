//! Outbound notification + task helpers used by the in-process
//! teammate loop.
//!
//! Extracted from `runner_loop.rs` (P1 split). All four helpers are
//! pure-ish — they format mailbox envelopes / prompts and write to
//! the team-lead inbox. No interior state.

use crate::constants::TEAM_LEAD_NAME;
use crate::mailbox;

/// Send a freeform message from a teammate to the team lead's inbox.
pub fn send_message_to_leader(
    from: &str,
    text: &str,
    color: Option<&str>,
    team_name: &str,
) -> crate::Result<()> {
    let message = mailbox::TeammateMessage {
        from: from.to_string(),
        text: text.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        read: false,
        color: color.map(String::from),
        summary: None,
    };
    mailbox::write_to_mailbox(TEAM_LEAD_NAME, message, team_name)
}

/// Send an idle notification to the leader.
pub fn send_idle_notification(
    agent_name: &str,
    color: Option<&str>,
    team_name: &str,
    idle_reason: Option<&str>,
    summary: Option<&str>,
) -> crate::Result<()> {
    let idle_text = mailbox::create_idle_notification(agent_name, idle_reason, summary);
    let message = mailbox::TeammateMessage {
        from: agent_name.to_string(),
        text: idle_text,
        timestamp: chrono::Utc::now().to_rfc3339(),
        read: false,
        color: color.map(String::from),
        summary: Some("idle notification".to_string()),
    };
    mailbox::write_to_mailbox(TEAM_LEAD_NAME, message, team_name)
}

/// Format a task as a prompt string.
pub fn format_task_as_prompt(task_id: &str, subject: &str, description: &str) -> String {
    let mut prompt = format!("Complete all open tasks. Start with task #{task_id}: \n\n {subject}");
    if !description.is_empty() {
        prompt.push_str(&format!("\n\n{description}"));
    }
    prompt
}

/// Find the first available (unclaimed) task from a list.
pub fn find_available_task(
    tasks: &[coco_types::TaskEntry],
) -> Option<(usize, &coco_types::TaskEntry)> {
    tasks
        .iter()
        .enumerate()
        .find(|(_, t)| t.status == "pending" && t.owner.is_none() && t.blocked_by.is_empty())
}
