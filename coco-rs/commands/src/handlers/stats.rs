//! `/stats` — show session statistics: count, duration, git changes, cwd.
//!
//! Reads session files from `~/.coco/sessions/`, inspects the most recent
//! file's creation time, counts all sessions, runs `git status --porcelain`
//! to count file changes, and reports the current working directory.

use std::pin::Pin;

/// Async handler for `/stats`.
pub fn handler(
    _args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        let sessions_dir = coco_config::global_config::config_home().join("sessions");

        let (session_count, session_start_secs) = scan_sessions(&sessions_dir).await;
        let duration_str = format_duration(session_start_secs);
        let git_changes = collect_git_changes().await;
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let mut out = String::from("## Session Statistics\n\n");

        out.push_str(&format!("  Sessions total:       {session_count}\n"));
        out.push_str(&format!("  Current session:      {duration_str}\n"));
        out.push_str(&format!("  Working directory:    {cwd}\n"));

        out.push_str("\n### Git Changes\n\n");
        match git_changes {
            Some(changes) => {
                out.push_str(&format!("  Modified (unstaged):  {}\n", changes.modified));
                out.push_str(&format!("  Staged:               {}\n", changes.staged));
                out.push_str(&format!("  Untracked:            {}\n", changes.untracked));
            }
            None => {
                out.push_str("  Not a git repository.\n");
            }
        }

        Ok(out)
    })
}

/// Counts of git file changes from `git status --porcelain`.
struct GitChanges {
    modified: i64,
    staged: i64,
    untracked: i64,
}

/// Scan sessions directory: returns (total_count, start_time_secs_of_newest).
///
/// The "start time" approximation uses the newest session file's modification
/// time, which is the closest available signal without parsing session JSON.
async fn scan_sessions(sessions_dir: &std::path::Path) -> (i64, u64) {
    if !sessions_dir.exists() {
        return (0, 0);
    }

    let Ok(mut entries) = tokio::fs::read_dir(sessions_dir).await else {
        return (0, 0);
    };

    let mut count: i64 = 0;
    let mut newest_modified: u64 = 0;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        count += 1;

        let modified = entry
            .metadata()
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());

        if modified > newest_modified {
            newest_modified = modified;
        }
    }

    (count, newest_modified)
}

/// Format a session duration given the session start timestamp (seconds since epoch).
///
/// Returns a human-readable string like "1h23m" or "45m" or "just started".
fn format_duration(start_secs: u64) -> String {
    if start_secs == 0 {
        return "unknown".to_string();
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());

    let elapsed = now.saturating_sub(start_secs);

    if elapsed < 60 {
        return "just started".to_string();
    }

    let hours = elapsed / 3600;
    let mins = (elapsed % 3600) / 60;

    if hours > 0 {
        format!("{hours}h{mins:02}m")
    } else {
        format!("{mins}m")
    }
}

/// Parse `git status --porcelain` output into change counts.
///
/// Returns `None` when not inside a git repository.
async fn collect_git_changes() -> Option<GitChanges> {
    let output = tokio::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut modified: i64 = 0;
    let mut staged: i64 = 0;
    let mut untracked: i64 = 0;

    for line in stdout.lines() {
        if line.len() < 2 {
            continue;
        }
        let index = line.chars().next().unwrap_or(' ');
        let worktree = line.chars().nth(1).unwrap_or(' ');

        if index == '?' && worktree == '?' {
            untracked += 1;
        } else {
            if index != ' ' {
                staged += 1;
            }
            if worktree != ' ' {
                modified += 1;
            }
        }
    }

    Some(GitChanges {
        modified,
        staged,
        untracked,
    })
}

#[cfg(test)]
#[path = "stats.test.rs"]
mod tests;
