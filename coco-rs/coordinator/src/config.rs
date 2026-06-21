//! Team/swarm configuration types.
//!
//! Whether the swarm subsystem is **active** is gated upstream by
//! `Feature::AgentTeams`; this struct only carries internal parameters
//! (mode, max agents, default model, etc.).

/// How teammates are spawned.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum TeammateMode {
    /// Auto-detect: try tmux/iTerm2 first, fall back to in-process.
    #[default]
    Auto,
    /// Force tmux backend.
    Tmux,
    /// Force in-process backend.
    InProcess,
}

impl TeammateMode {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Tmux => "tmux",
            Self::InProcess => "in-process",
        }
    }
}

/// Team/swarm configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TeamConfig {
    /// How to spawn teammates.
    #[serde(default)]
    pub teammate_mode: TeammateMode,

    /// Default role for new teammates. Missing config routes through Main.
    #[serde(default = "default_model_role")]
    pub default_model_role: coco_types::ModelRole,

    /// Per agent-type role overrides.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub agent_type_model_roles: std::collections::HashMap<String, coco_types::ModelRole>,

    /// Show spinner tree instead of pills.
    #[serde(default = "default_true")]
    pub show_spinner_tree: bool,

    /// Maximum concurrent in-process agents.
    #[serde(default = "default_max_agents")]
    pub max_agents: i32,
}

fn default_true() -> bool {
    true
}

fn default_max_agents() -> i32 {
    8
}

fn default_model_role() -> coco_types::ModelRole {
    coco_types::ModelRole::Main
}

impl Default for TeamConfig {
    fn default() -> Self {
        Self {
            teammate_mode: TeammateMode::Auto,
            default_model_role: coco_types::ModelRole::Main,
            agent_type_model_roles: std::collections::HashMap::new(),
            show_spinner_tree: true,
            max_agents: default_max_agents(),
        }
    }
}

#[cfg(test)]
#[path = "config.test.rs"]
mod tests;
