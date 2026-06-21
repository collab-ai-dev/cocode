//! Swarm constants.

use coco_config::EnvKey;

/// Name used for the team leader agent.
pub const TEAM_LEAD_NAME: &str = "team-lead";

/// Tmux session name for the swarm coordinator.
pub const SWARM_SESSION_NAME: &str = "claude-swarm";

/// Tmux window name for the swarm view layout.
pub const SWARM_VIEW_WINDOW_NAME: &str = "swarm-view";

/// Tmux session name for hidden panes.
pub const HIDDEN_SESSION_NAME: &str = "claude-hidden";

/// Tmux command name.
pub const TMUX_COMMAND: &str = "tmux";

// Swarm + plan-mode env vars use the `COCO_` prefix (coco-rs native).

/// Env var: override command used to spawn teammates.
pub const TEAMMATE_COMMAND_ENV_VAR: EnvKey = EnvKey::CocoTeammateCommand;

/// Env var: teammate's assigned UI color.
pub const TEAMMATE_COLOR_ENV_VAR: EnvKey = EnvKey::CocoAgentColor;

/// Env var: force plan mode for teammates.
pub const PLAN_MODE_REQUIRED_ENV_VAR: EnvKey = EnvKey::CocoPlanModeRequired;

/// Env var: teammate's agent ID (cross-process identity fallback).
pub const AGENT_ID_ENV_VAR: EnvKey = EnvKey::CocoAgentId;

/// Env var: teammate's human-facing agent name.
pub const AGENT_NAME_ENV_VAR: EnvKey = EnvKey::CocoAgentName;

/// Env var: teammate's team name.
pub const TEAM_NAME_ENV_VAR: EnvKey = EnvKey::CocoTeamName;

/// Env var: parent session ID, piped from the leader so teammates can
/// correlate cross-process logs + replay.
pub const PARENT_SESSION_ID_ENV_VAR: EnvKey = EnvKey::CocoParentSessionId;

/// Env var: opt into the VerifyPlanExecution PostToolUse hook that
/// compares plan-file mtime vs `plan_mode_entry_ms`.
pub const VERIFY_PLAN_ENV_VAR: EnvKey = EnvKey::CocoVerifyPlan;

/// Generate a swarm socket name based on the current PID.
pub fn swarm_socket_name() -> String {
    format!("claude-swarm-{}", std::process::id())
}

// `AgentColorName` lives in `coco_types::agent::AgentColorName` (the
// canonical home — also used by `core/subagent`). Re-exported here so
// the existing `crate::constants::AgentColorName` paths stay short.
pub use coco_types::AgentColorName;

#[cfg(test)]
#[path = "constants.test.rs"]
mod tests;
