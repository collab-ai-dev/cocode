//! Team discovery — enumerate teams and teammate statuses.

use crate::team_file;
use crate::types::BackendType;

/// Summary of a team.
#[derive(Debug, Clone)]
pub struct TeamSummary {
    pub name: String,
    pub member_count: i32,
    pub running_count: i32,
    pub idle_count: i32,
}

/// Status of a single teammate.
#[derive(Debug, Clone)]
pub struct TeammateStatus {
    pub name: String,
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub status: TeammateRunStatus,
    pub color: Option<String>,
    pub idle_since: Option<String>,
    pub tmux_pane_id: String,
    pub cwd: String,
    pub worktree_path: Option<String>,
    pub is_hidden: bool,
    pub backend_type: Option<BackendType>,
    pub mode: Option<String>,
}

/// Runtime status of a teammate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeammateRunStatus {
    Running,
    Idle,
    Unknown,
}

impl TeammateRunStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Unknown => "unknown",
        }
    }
}

/// Get teammate statuses for a team by reading the team file.
pub fn get_teammate_statuses(team_name: &str) -> Vec<TeammateStatus> {
    let Some(team_file) = team_file::read_team_file(team_name).ok().flatten() else {
        return Vec::new();
    };

    team_file
        .members
        .iter()
        .map(|m| {
            let status = if m.is_active {
                TeammateRunStatus::Running
            } else {
                TeammateRunStatus::Idle
            };

            let is_hidden = team_file.hidden_pane_ids.contains(&m.tmux_pane_id);

            TeammateStatus {
                name: m.name.clone(),
                agent_id: m.agent_id.clone(),
                agent_type: m.agent_type.clone(),
                model: m.model.clone(),
                prompt: m.prompt.clone(),
                status,
                color: m.color.clone(),
                idle_since: None,
                tmux_pane_id: m.tmux_pane_id.clone(),
                cwd: m.cwd.clone(),
                worktree_path: m.worktree_path.clone(),
                is_hidden,
                backend_type: m.backend_type,
                mode: m.mode.map(|m| format!("{m:?}")),
            }
        })
        .collect()
}

/// Get a summary of a team.
pub fn get_team_summary(team_name: &str) -> Option<TeamSummary> {
    let tf = team_file::read_team_file(team_name).ok()??;
    let running = tf.members.iter().filter(|m| m.is_active).count() as i32;
    let idle = tf.members.len() as i32 - running;

    Some(TeamSummary {
        name: tf.name,
        member_count: tf.members.len() as i32,
        running_count: running,
        idle_count: idle,
    })
}

/// List all teams with their summaries.
pub fn list_teams() -> Vec<TeamSummary> {
    team_file::list_team_names()
        .into_iter()
        .filter_map(|name| get_team_summary(&name))
        .collect()
}

#[cfg(test)]
#[path = "discovery.test.rs"]
mod tests;
