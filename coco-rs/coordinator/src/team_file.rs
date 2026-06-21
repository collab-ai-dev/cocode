//! Team file I/O — disk persistence for team configuration.
//!
//! File layout:
//! ```text
//! ~/.coco/teams/
//!   {team-name}/
//!     config.json       # TeamFile
//!     inboxes/
//!       {agent-name}.json  # TeammateMessage[]
//!     permissions/
//!       pending/
//!       resolved/
//! ```

use std::path::PathBuf;

use crate::types::TeamFile;

/// Base directory for all teams.
///
/// `COCO_TEAMS_DIR` overrides it (tests isolate the teams/mailbox tree; a
/// future swarm-leader can relocate it). Otherwise `~/.coco/teams/`.
pub fn teams_base_dir() -> PathBuf {
    if let Some(dir) = coco_config::env::env_opt(coco_config::EnvKey::CocoTeamsDir) {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".coco")
        .join("teams")
}

/// Get the team directory for a given team name.
pub fn get_team_dir(team_name: &str) -> PathBuf {
    teams_base_dir().join(crate::types::sanitize_name(team_name))
}

/// Get the team file path for a given team name.
pub fn get_team_file_path(team_name: &str) -> PathBuf {
    get_team_dir(team_name).join("config.json")
}

/// Read a team file from disk.
pub fn read_team_file(team_name: &str) -> crate::Result<Option<TeamFile>> {
    let path = get_team_file_path(team_name);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)?;
    let team_file: TeamFile = serde_json::from_str(&content)?;
    Ok(Some(team_file))
}

/// Write a team file to disk.
pub fn write_team_file(team_name: &str, team_file: &TeamFile) -> crate::Result<()> {
    let path = get_team_file_path(team_name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(team_file)?;
    std::fs::write(&path, content)?;
    Ok(())
}

/// Write a single member's permission mode into `team.json`.
///
/// Free-function form of [`crate::roster_store::TeamRosterStore::set_member_mode`]
/// for the **teammate** side, which has no leader-side roster manager to
/// upsert. Used when a (cross-process) teammate self-cycles or applies a
/// `ModeSetRequest`, so the leader's roster view reflects the live mode.
/// No-op when the team file or member is missing.
pub fn set_member_mode_in_team_file(
    team_name: &str,
    member_name: &str,
    mode: coco_types::PermissionMode,
) -> crate::Result<()> {
    let Some(mut team_file) = read_team_file(team_name)? else {
        return Ok(());
    };
    if let Some(member) = team_file.members.iter_mut().find(|m| m.name == member_name) {
        member.mode = Some(mode);
        write_team_file(team_name, &team_file)?;
    }
    Ok(())
}

/// Remove a teammate from the team file by agent ID.
pub(crate) fn remove_member_by_agent_id(team_name: &str, agent_id: &str) -> crate::Result<bool> {
    let Some(mut team_file) = read_team_file(team_name)? else {
        return Ok(false);
    };
    let before = team_file.members.len();
    team_file.members.retain(|m| m.agent_id != agent_id);
    if team_file.members.len() < before {
        write_team_file(team_name, &team_file)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// List all known team names (from the teams directory).
pub fn list_team_names() -> Vec<String> {
    let base = teams_base_dir();
    if !base.is_dir() {
        return Vec::new();
    }
    let entries = match std::fs::read_dir(&base) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            // Verify config.json exists
            if e.path().join("config.json").exists() {
                Some(name)
            } else {
                None
            }
        })
        .collect()
}

/// Outcome of [`cleanup_team_directories`].
///
/// `tasks_dir_removed` gates callers' task-list change notification:
/// `notifyTasksUpdated()` fires **only** after `rm(tasksDir)` succeeds
/// (the `catch` path does not notify). Callers that own a task-list change
/// notifier (e.g. [`crate::roster_store::TeamRosterStore::delete_team`])
/// fire it iff this flag is `true`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanupOutcome {
    /// `true` when the team's task-list directory existed and was removed
    /// (or did not exist — nothing orphaned). `false` only when the
    /// removal was attempted and failed.
    pub tasks_dir_removed: bool,
}

/// Clean up everything a team owns: per-member worktrees, the team dir,
/// and the team's task-list directory.
///
/// Destroys each `member.worktreePath`, then removes the team dir, then
/// removes `getTasksDir(sanitizedName)`. The worktree + tasks-dir steps are
/// best-effort: a failure there is logged and must not abort the team-dir
/// removal.
///
/// Returns [`CleanupOutcome::tasks_dir_removed`] so the caller can fire a
/// task-list change notification on the success path only. The notification
/// itself is wired at the caller (it owns the task-list handle); this
/// low-level file-IO helper stays dependency-free.
pub fn cleanup_team_directories(team_name: &str) -> crate::Result<CleanupOutcome> {
    // 1. Destroy each member's git worktree. Read the team file BEFORE the
    //    dir is removed below; skip members without an isolated worktree.
    if let Ok(Some(team_file)) = read_team_file(team_name) {
        for member in &team_file.members {
            if let Some(worktree_path) = &member.worktree_path
                && let Err(e) = destroy_worktree(worktree_path)
            {
                tracing::warn!(
                    member = %member.name,
                    worktree = %worktree_path,
                    error = %e,
                    "team cleanup: failed to destroy worktree (continuing)"
                );
            }
        }
    }

    // 2. Remove the team dir and its contents.
    let dir = get_team_dir(team_name);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir)?;
    }

    // 3. Remove the team's task-list directory, which would otherwise be
    //    orphaned. The store's `task_list_id` is `sanitize_name(team_name)`
    //    (roster_store) and lives at `{config_home}/tasks/{sanitized}`
    //    (`TaskList::open`). Best-effort so a tasks-dir failure never blocks
    //    a delete.
    let task_list_id = crate::types::sanitize_name(team_name);
    let tasks_dir = coco_config::global_config::config_home()
        .join("tasks")
        .join(coco_tasks::task_list::sanitize_path_component(
            &task_list_id,
        ));
    // `tasks_dir_removed` stays `true` when the dir never existed (nothing
    // orphaned) and flips to `false` only when an attempted removal failed
    // — that flag gates the caller's task-list change notification.
    let mut tasks_dir_removed = true;
    if tasks_dir.is_dir()
        && let Err(e) = std::fs::remove_dir_all(&tasks_dir)
    {
        tasks_dir_removed = false;
        tracing::warn!(
            dir = %tasks_dir.display(),
            error = %e,
            "team cleanup: failed to remove task-list dir (continuing)"
        );
    }

    Ok(CleanupOutcome { tasks_dir_removed })
}

/// Clean up teams owned by the current session.
///
/// On leader exit (graceful or SIGINT/crash), for each team this session
/// led: kill any still-running teammate panes FIRST (otherwise the child
/// `coco` processes orphan — gh-32730 class), THEN remove the team dir +
/// worktrees + tasks. Pane-kill must precede dir removal because it reads
/// the member list out of the team file.
pub fn cleanup_session_teams(session_id: &str) -> crate::Result<()> {
    for name in list_team_names() {
        if let Ok(Some(tf)) = read_team_file(&name)
            && tf.lead_session_id.as_deref() == Some(session_id)
        {
            // Best-effort: a pane-kill failure must not block dir cleanup.
            if let Err(e) = kill_orphaned_teammate_panes(&name) {
                tracing::warn!(team = %name, error = %e,
                    "session cleanup: failed to kill orphaned teammate panes (continuing)");
            }
            // Shutdown path: no live task-list subscriber to notify (the
            // session UI is already torn down), so the cleanup outcome is
            // discarded. The roster-store delete path is the one that fires
            // the change notification.
            cleanup_team_directories(&name)?;
        }
    }
    Ok(())
}

/// Destroy a git worktree.
///
/// Runs `git worktree remove`, falls back to rm.
pub fn destroy_worktree(worktree_path: &str) -> crate::Result<()> {
    let path = std::path::Path::new(worktree_path);
    if !path.exists() {
        return Ok(());
    }
    // Try git worktree remove first
    let status = std::process::Command::new("git")
        .args(["worktree", "remove", "--force", worktree_path])
        .status();
    if status.is_ok_and(|s| s.success()) {
        return Ok(());
    }
    // Fallback: remove directory
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// Kill all tmux-pane teammates for a team (orphan cleanup on leader
/// exit). Best-effort: a failed kill is ignored so cleanup proceeds.
///
/// Only tmux panes are torn down here via the `tmux` CLI. iTerm2 panes
/// need the it2 backend, which is reached on the graceful shutdown path
/// (`AgentHandle::teardown_teammate` → registry pane backend); an iTerm2
/// pane left by a crash is a documented follow-up, not killed with the
/// wrong backend.
pub fn kill_orphaned_teammate_panes(team_name: &str) -> crate::Result<()> {
    let Some(team_file) = read_team_file(team_name)? else {
        return Ok(());
    };
    for member in &team_file.members {
        if member.tmux_pane_id.is_empty()
            || member.backend_type != Some(crate::types::BackendType::Tmux)
        {
            continue;
        }
        let _ = std::process::Command::new("tmux")
            .args(["kill-pane", "-t", &member.tmux_pane_id])
            .status();
    }
    Ok(())
}

#[cfg(test)]
#[path = "team_file.test.rs"]
mod tests;
