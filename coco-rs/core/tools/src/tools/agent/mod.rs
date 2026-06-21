//! Agent / Skill / SendMessage / TeamCreate / TeamDelete tools.
//!
//! One submodule per tool, all sitting under this
//! `agent/` parent so the existing `pub mod agent;` re-export in
//! `tools/mod.rs` keeps working.
//!
//! These structs are schema/validation/result-formatting wrappers only.
//! The AgentTool dispatches to `ToolUseContext.agent` (AgentHandle trait)
//! to spawn subagents, avoiding circular dependencies between tools and
//! the spawning infrastructure.
//!
//! Pure-logic helpers (definition catalog, prompt rendering, tool-filter
//! planning, fork-context construction, transcript filtering) live in
//! `coco-subagent`. Spawn lifecycle, mailbox IPC, terminal backends, and
//! the runner live in `coco-coordinator`. This module only builds
//! `AgentSpawnRequest` and forwards to `AgentHandle::spawn_agent`.

pub mod agent_tool;
pub mod send_message_tool;
pub mod skill_tool;
pub mod team_tools;

pub use agent_tool::AgentTool;
pub use send_message_tool::SendMessageTool;
pub use skill_tool::SkillTool;
pub use team_tools::TeamCreateTool;
pub use team_tools::TeamDeleteTool;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
