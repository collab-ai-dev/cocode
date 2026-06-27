//! Agent / Skill / SendMessage tools.
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

pub use agent_tool::AgentTool;
pub use send_message_tool::SendMessageTool;
pub use skill_tool::SkillTool;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
