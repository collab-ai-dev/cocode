//! Per-engine `PermissionRuleHandle` for skill-emitted Command-source
//! rules.
//!
//! # Scope
//!
//! Skill `allowed-tools` rules accumulate in
//! `appState.alwaysAllowRules.command` for **one `query()` call**:
//!
//! - **Within one user message** — every turn of that `query()` invocation
//!   sees rules accumulated by earlier turns (cross-turn propagation).
//! - **Across user messages** — `query()` returns, the closure's captured
//!   `appState` is GC'd, the next user message's `query()` starts fresh.
//!
//! In coco-rs, `QueryEngine` is rebuilt per user message
//! (`SessionRuntime::build_engine`, called from `tui_runner::run_user_message_turn`,
//! `headless::run`, `sdk_runner`, `fork_dispatcher`). So **engine-scoped
//! state is user-message-scoped state**. This handle owns an
//! `Arc<RwLock<Vec<PermissionRule>>>` shared with the engine and
//! [`crate::tool_context::ToolContextFactory`]: skills push rules in,
//! the factory merges them into [`coco_types::ToolPermissionContext::allow_rules`]
//! at the `Command` source on each batch's context build, and the
//! whole structure is released when the engine drops.
//!
//! # Subagent isolation
//!
//! Each subagent fork builds its own `QueryEngine` via
//! `SessionRuntime::build_engine_from_config` → its own
//! `live_command_rules` Arc → its own handle. Rules emitted inside a
//! subagent's skill **cannot** leak to the parent — different Arcs.
//! No explicit NoOp override needed; per-engine isolation is the
//! default.
//!
//! # What this handle does NOT do
//!
//! Updates whose [`PermissionUpdateDestination`] is not `Command` are
//! dropped with a `tracing::debug!`. Disk-persisting destinations
//! (`UserSettings` / `ProjectSettings` / `LocalSettings`) and the
//! session-config-mutating `Session` / `CliArg` destinations have
//! their own paths (TUI "Always Allow" dialog, `/permissions` slash
//! command). Skills today only emit `Command` so this is a hard
//! contract, not a TODO.

use std::sync::Arc;

use async_trait::async_trait;
use coco_tool_runtime::PermissionRuleHandle;
use coco_types::PermissionRule;
use coco_types::PermissionUpdate;
use coco_types::PermissionUpdateDestination;
use tokio::sync::RwLock;

/// Per-engine handle: writes skill-emitted Command-source rules into
/// a `Vec<PermissionRule>` shared by `Arc` with the engine and the
/// `ToolContextFactory`. See module docs for lifecycle.
pub struct EngineLiveRulesHandle {
    /// Shared with `QueryEngine.live_command_rules` and
    /// `ToolContextFactory.live_command_rules`. Resets when the engine
    /// drops.
    live_rules: Arc<RwLock<Vec<PermissionRule>>>,
}

impl EngineLiveRulesHandle {
    pub fn new(live_rules: Arc<RwLock<Vec<PermissionRule>>>) -> Self {
        Self { live_rules }
    }
}

#[async_trait]
impl PermissionRuleHandle for EngineLiveRulesHandle {
    async fn apply_updates(&self, updates: Vec<PermissionUpdate>) {
        if updates.is_empty() {
            return;
        }
        let total_updates = updates.len();
        let mut to_add: Vec<PermissionRule> = Vec::new();
        let mut dropped_non_command = 0usize;
        for update in updates {
            match update {
                PermissionUpdate::AddRules {
                    rules,
                    destination: PermissionUpdateDestination::Command,
                } => to_add.extend(rules),
                other => {
                    dropped_non_command += 1;
                    tracing::debug!(
                        destination = ?other.destination(),
                        "engine_live_rules: dropping non-Command-AddRules update"
                    );
                }
            }
        }
        if to_add.is_empty() {
            tracing::debug!(
                total_updates,
                dropped_non_command,
                "engine_live_rules: apply_updates produced no Command-source rules"
            );
            return;
        }
        let patterns: Vec<&str> = to_add
            .iter()
            .map(|r| r.value.tool_pattern.as_str())
            .collect();
        let mut guard = self.live_rules.write().await;
        let before = guard.len();
        guard.extend(to_add.iter().cloned());
        let after = guard.len();
        // info: a security-meaningful state change (a skill just
        // widened the permission surface for the rest of this user
        // message). Ops should be able to grep this.
        tracing::info!(
            added = after - before,
            total_after = after,
            patterns = ?patterns,
            dropped_non_command,
            "engine_live_rules: appended Command-source allow rules to live overlay"
        );
    }
}

#[cfg(test)]
#[path = "engine_live_rules.test.rs"]
mod tests;
