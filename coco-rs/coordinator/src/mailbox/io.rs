//! On-disk I/O for teammate mailboxes.
//!
//! Inbox layout: `~/.coco/teams/{team_name}/inboxes/{agent_name}.json`.
//! Each file is a JSON array of [`TeammateMessage`]. Concurrent writes
//! are serialised by the advisory lock helpers in
//! [`super::lock::with_inbox_lock`].

use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::team_file::get_team_dir;

use super::lock::{read_messages_no_lock, with_inbox_lock};

// ── Core Message Type ──

/// A message in the teammate mailbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeammateMessage {
    pub from: String,
    pub text: String,
    pub timestamp: String,
    #[serde(default)]
    pub read: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Brief preview (5-10 words).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

// ── Inbox Path ──

/// Get the inbox directory for a team.
fn inbox_dir(team_name: &str) -> PathBuf {
    get_team_dir(team_name).join("inboxes")
}

/// Get the inbox file path for a specific agent.
pub fn inbox_path(agent_name: &str, team_name: &str) -> PathBuf {
    inbox_dir(team_name).join(format!("{agent_name}.json"))
}

// ── Read / Write ──

/// Read all messages from an agent's inbox.
pub fn read_mailbox(agent_name: &str, team_name: &str) -> crate::Result<Vec<TeammateMessage>> {
    let path = inbox_path(agent_name, team_name);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    let messages: Vec<TeammateMessage> = serde_json::from_str(&content)?;
    Ok(messages)
}

/// Read only unread messages.
pub fn read_unread_messages(
    agent_name: &str,
    team_name: &str,
) -> crate::Result<Vec<TeammateMessage>> {
    let all = read_mailbox(agent_name, team_name)?;
    Ok(all.into_iter().filter(|m| !m.read).collect())
}

/// Write a message to a recipient's inbox.
///
/// Uses `fs2`'s advisory exclusive lock on a sidecar `.lock` file with
/// 10 retries and 5-100 ms exponential backoff; concurrent writers spin
/// until they acquire it. Read-after-lock prevents the classic TOCTOU of
/// "read-mailbox → append → write" losing a concurrent peer's message.
pub fn write_to_mailbox(
    recipient_name: &str,
    message: TeammateMessage,
    team_name: &str,
) -> crate::Result<()> {
    let path = inbox_path(recipient_name, team_name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    with_inbox_lock(&path, |path| {
        // Inside the lock: read-current, append, write. The outer lock
        // serializes this RMW cycle against concurrent writers.
        let mut messages = read_messages_no_lock(path).unwrap_or_default();
        messages.push(message.clone());
        let content = serde_json::to_string_pretty(&messages)?;
        std::fs::write(path, content)?;
        Ok(())
    })
}

/// Mark all messages as read.
///
/// Read-modify-write under `with_inbox_lock` — same TOCTOU avoidance as
/// [`write_to_mailbox`]. An unlocked RMW here could clobber a peer's
/// `write_to_mailbox` that appended between this fn's read and write-back.
pub fn mark_messages_as_read(agent_name: &str, team_name: &str) -> crate::Result<()> {
    let path = inbox_path(agent_name, team_name);
    if !path.exists() {
        return Ok(());
    }
    with_inbox_lock(&path, |path| {
        let mut messages = read_messages_no_lock(path).unwrap_or_default();
        for msg in &mut messages {
            msg.read = true;
        }
        let content = serde_json::to_string_pretty(&messages)?;
        std::fs::write(path, content)?;
        Ok(())
    })
}

/// Mark a message as read by index.
///
/// Locked RMW (see [`mark_messages_as_read`]). The append-only inbox keeps
/// any earlier `index` valid even if a peer appended concurrently — and the
/// in-lock re-read preserves that appended message instead of dropping it.
pub fn mark_message_as_read_by_index(
    agent_name: &str,
    team_name: &str,
    index: usize,
) -> crate::Result<()> {
    let path = inbox_path(agent_name, team_name);
    if !path.exists() {
        return Ok(());
    }
    with_inbox_lock(&path, |path| {
        let mut messages = read_messages_no_lock(path).unwrap_or_default();
        if let Some(msg) = messages.get_mut(index) {
            msg.read = true;
            let content = serde_json::to_string_pretty(&messages)?;
            std::fs::write(path, content)?;
        }
        Ok(())
    })
}

/// Mark messages as read by predicate.
///
/// Locked RMW (see [`mark_messages_as_read`]).
pub fn mark_messages_as_read_by_predicate(
    agent_name: &str,
    team_name: &str,
    predicate: impl Fn(&TeammateMessage) -> bool,
) -> crate::Result<()> {
    let path = inbox_path(agent_name, team_name);
    if !path.exists() {
        return Ok(());
    }
    with_inbox_lock(&path, |path| {
        let mut messages = read_messages_no_lock(path).unwrap_or_default();
        for msg in &mut messages {
            if predicate(msg) {
                msg.read = true;
            }
        }
        let content = serde_json::to_string_pretty(&messages)?;
        std::fs::write(path, content)?;
        Ok(())
    })
}

/// Clear an agent's inbox.
pub fn clear_mailbox(agent_name: &str, team_name: &str) -> crate::Result<()> {
    let path = inbox_path(agent_name, team_name);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Format teammate messages for display in the conversation.
pub fn format_teammate_messages(messages: &[TeammateMessage]) -> String {
    messages
        .iter()
        .map(|m| {
            let color_attr = m
                .color
                .as_deref()
                .map(|c| format!(" color=\"{c}\""))
                .unwrap_or_default();
            let summary_attr = m
                .summary
                .as_deref()
                .map(|s| format!(" summary=\"{s}\""))
                .unwrap_or_default();
            format!(
                "<teammate_message teammate_id=\"{from}\"{color_attr}{summary_attr}>\n{text}\n</teammate_message>",
                from = m.from,
                text = m.text,
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}
