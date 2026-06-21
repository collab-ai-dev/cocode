//! Spawn lifecycle for the agent-team subsystem.
//!
//! Owns: runner, runner-loop, mailbox IPC, team file, terminal pane
//! backends (tmux / iTerm2 / pane executor), agent identity / discovery,
//! and the [`coco_tool_runtime::AgentHandle`] implementation
//! the tool layer invokes.
//!
//! Pure-logic rules and templates live in `core/subagent`
//! (definitions, prompt rendering, tool filter, fork helpers, transcript
//! filter, coordinator-mode prompt). This crate is the orchestration
//! layer that calls into those helpers.
//!
//! See `docs/coco-rs/agentteam-architecture.md` for the multi-PR plan
//! that produced this crate.

// ── Public surface ──
//
// External callers import via these paths today:
// - `coco_coordinator::agent_handle::SwarmAgentHandle`
// - `coco_coordinator::mailbox::SwarmMailboxHandle`
// - `coco_coordinator::runner::{InProcessAgentRunner, PermissionBridge}`
// - `coco_coordinator::types::TeamManager`
// - `coco_coordinator::runner_loop::{wait_for_plan_approval, MailboxPermissionBridge,
//   AgentExecutionEngine}` (consumed by the production engine adapter)
//
// These modules stay `pub` for stable callers. Implementation-only
// modules below are `pub(crate)` so they aren't part of the version
// contract — they can move freely without touching downstream code.

/// Bridge between the tool layer's [`coco_tool_runtime::AgentHandle`]
/// trait and the swarm's runner/mailbox/team-file infrastructure.
///
/// Internally split into `mod.rs` (struct + trait impl + teammate
/// dispatch), `spawn.rs` (subagent dispatch — sync + background),
/// `handoff.rs` (post-spawn classifier + AgentSummary), `resume.rs`
/// (background-spawn resume).
mod error;
pub use error::CoordinatorError;
pub use error::Result;

pub mod agent_handle;
/// File-based teammate inboxes (`~/.coco/teams/{team}/inboxes/{agent}.json`)
/// + structured protocol message envelopes.
pub mod mailbox;
/// Pane-backend trait + tmux / iTerm2 / in-process / it2-setup impls.
pub mod pane;
/// In-process runner, permission bridge channel, agent context types.
pub mod runner;
/// Per-iteration teammate execution loop. Helpers split into sibling
/// modules (P1): see `runner_loop_mailbox_permission`,
/// `runner_loop_wait`, `runner_loop_notify`.
pub mod runner_loop;
/// Cross-process worker permission via mailbox file IPC. P1 split
/// from `runner_loop`. Hosts `request_permission_via_mailbox` +
/// `MailboxPermissionBridge`.
pub mod runner_loop_mailbox_permission;
/// Outbound mailbox notification helpers used by the in-process
/// teammate loop. P1 split from `runner_loop`.
pub mod runner_loop_notify;
/// Plan-approval mailbox waiter. P1 split from `runner_loop`.
pub mod runner_loop_wait;
/// Team file r/w (`~/.coco/teams/{team}/team.json`).
pub mod team_file;
/// `BackendType` + cross-cutting team / teammate / standalone-agent
/// types shared between the coordinator and AppState consumers.
pub mod types;
/// Worktree-isolated subagent management.
pub mod worktree;

// ── Implementation modules — kept `pub` for now to preserve external
// path-based imports, but consumers should treat the crate-root
// re-exports below as the stable surface. ──
pub mod config;
pub mod constants;
pub mod discovery;
pub mod identity;
pub mod inprocess_backend;
pub mod prompt;
pub mod roster_store;
pub mod spawn;
pub mod teammate;

#[cfg(test)]
mod test_support;

// ── Crate-root re-exports for the stable surface. ──

pub use agent_handle::SwarmAgentHandle;
pub use inprocess_backend::InProcessBackend;
pub use mailbox::SwarmMailboxHandle;
pub use runner::InProcessAgentRunner;
pub use runner_loop::AgentExecutionEngine;
pub use runner_loop_mailbox_permission::MailboxPermissionBridge;
pub use runner_loop_wait::wait_for_plan_approval;
pub use types::{BackendType, TeamManager};
