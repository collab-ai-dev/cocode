//! `/session` — list, resume, and delete sessions with real file I/O.
//!
//! Reads session files from `~/.coco/sessions/`, parses their metadata
//! (timestamps, model, working directory), and formats a listing.

use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;

/// Parsed metadata from a session file.
struct SessionInfo {
    id: String,
    path: PathBuf,
    modified_secs: u64,
    model: Option<String>,
    working_dir: Option<String>,
    message_count: i64,
}

/// Async handler for `/session [list|delete <id>|info <id>]`.
pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        let subcommand = args.trim().to_string();
        let sessions_dir = coco_config::global_config::config_home().join("sessions");

        match subcommand.as_str() {
            "" | "list" => list_sessions(&sessions_dir).await,
            "delete" => {
                Ok("Usage: /session delete <id>\n\nSpecify a session ID to delete.".to_string())
            }
            _ => {
                if let Some(id) = subcommand.strip_prefix("delete ") {
                    delete_session(&sessions_dir, id.trim()).await
                } else if let Some(id) = subcommand.strip_prefix("info ") {
                    session_info(&sessions_dir, id.trim()).await
                } else {
                    Ok(format!(
                        "Unknown session subcommand: {subcommand}\n\n\
                        Usage:\n  /session list\n  /session delete <id>\n  /session info <id>"
                    ))
                }
            }
        }
    })
}

/// List all sessions, sorted by modification time (newest first).
async fn list_sessions(sessions_dir: &Path) -> crate::Result<String> {
    if !sessions_dir.exists() {
        return Ok(
            "No sessions found.\n\nSessions are stored in ~/.coco/sessions/\n\
             Start a conversation to create your first session."
                .to_string(),
        );
    }

    let sessions = scan_sessions(sessions_dir).await;

    if sessions.is_empty() {
        return Ok("No session files found in ~/.coco/sessions/".to_string());
    }

    let mut out = format!("## Sessions ({} found)\n\n", sessions.len());

    for (i, session) in sessions.iter().take(25).enumerate() {
        let age = format_age(session.modified_secs);
        let model = session.model.as_deref().unwrap_or("-");
        let msgs = session.message_count;
        out.push_str(&format!(
            "  {: >2}. {:<24}  {age:<10}  {msgs:>3} msgs  {model}\n",
            i + 1,
            truncate_id(&session.id, 24),
        ));
    }

    if sessions.len() > 25 {
        out.push_str(&format!("\n  ... and {} more\n", sessions.len() - 25));
    }

    out.push_str("\nCommands:\n");
    out.push_str("  /resume <id>         Resume a session\n");
    out.push_str("  /session info <id>   Show session details\n");
    out.push_str("  /session delete <id> Delete a session");

    Ok(out)
}

/// Delete a session by ID (prefix match).
async fn delete_session(sessions_dir: &Path, id: &str) -> crate::Result<String> {
    let sessions = scan_sessions(sessions_dir).await;

    let matching: Vec<&SessionInfo> = sessions.iter().filter(|s| s.id.starts_with(id)).collect();

    match matching.len() {
        0 => Ok(format!("No session matching '{id}' found.")),
        1 => {
            let session = matching[0];
            tokio::fs::remove_file(&session.path).await?;
            Ok(format!("Deleted session: {}", session.id))
        }
        n => {
            let mut out = format!("Ambiguous: {n} sessions match '{id}':\n\n");
            for s in matching {
                out.push_str(&format!("  {}\n", s.id));
            }
            out.push_str("\nProvide a longer prefix to disambiguate.");
            Ok(out)
        }
    }
}

/// Show detailed info for a specific session.
async fn session_info(sessions_dir: &Path, id: &str) -> crate::Result<String> {
    let sessions = scan_sessions(sessions_dir).await;

    let session = sessions.iter().find(|s| s.id.starts_with(id));

    match session {
        Some(s) => {
            let mut out = format!("## Session: {}\n\n", s.id);
            out.push_str(&format!("  File:        {}\n", s.path.display()));
            out.push_str(&format!(
                "  Modified:    {} ago\n",
                format_age(s.modified_secs)
            ));
            out.push_str(&format!("  Messages:    {}\n", s.message_count));
            out.push_str(&format!(
                "  Model:       {}\n",
                s.model.as_deref().unwrap_or("unknown")
            ));
            out.push_str(&format!(
                "  Working dir: {}\n",
                s.working_dir.as_deref().unwrap_or("unknown")
            ));

            // File size
            if let Ok(meta) = tokio::fs::metadata(&s.path).await {
                let size_kb = meta.len() / 1024;
                out.push_str(&format!("  File size:   {size_kb} KB"));
            }

            Ok(out)
        }
        None => Ok(format!("No session matching '{id}' found.")),
    }
}

/// Scan the sessions directory and parse each JSON file.
async fn scan_sessions(sessions_dir: &Path) -> Vec<SessionInfo> {
    let Ok(mut entries) = tokio::fs::read_dir(sessions_dir).await else {
        return Vec::new();
    };

    let mut sessions = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let modified_secs = entry
            .metadata()
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());

        // Parse just enough to extract metadata
        let (model, working_dir, message_count) =
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                parse_session_metadata(&content)
            } else {
                (None, None, 0)
            };

        sessions.push(SessionInfo {
            id,
            path,
            modified_secs,
            model,
            working_dir,
            message_count,
        });
    }

    sessions.sort_by(|a, b| b.modified_secs.cmp(&a.modified_secs));
    sessions
}

/// Extract model, working_dir, and message count from session JSON.
fn parse_session_metadata(content: &str) -> (Option<String>, Option<String>, i64) {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) else {
        return (None, None, 0);
    };

    let model = parsed
        .get("model")
        .and_then(|v| v.as_str())
        .map(String::from);

    let working_dir = parsed
        .get("working_dir")
        .or_else(|| parsed.get("workingDirectory"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let message_count = parsed
        .get("messages")
        .or_else(|| parsed.get("message_history"))
        .and_then(|v| v.as_array())
        .map_or(0, |m| m.len() as i64);

    (model, working_dir, message_count)
}

/// Format seconds-since-epoch as a human-readable relative time.
fn format_age(modified_secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());

    let delta = now.saturating_sub(modified_secs);

    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        let mins = delta / 60;
        format!("{mins}m ago")
    } else if delta < 86400 {
        let hours = delta / 3600;
        format!("{hours}h ago")
    } else {
        let days = delta / 86400;
        format!("{days}d ago")
    }
}

/// Truncate a string to a max length, appending "..." if truncated.
fn truncate_id(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

#[cfg(test)]
#[path = "session.test.rs"]
mod tests;
