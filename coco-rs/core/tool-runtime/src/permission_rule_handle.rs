//! Permission-rule mutation callback.
//!
//! Tools (today: only `SkillTool`) can return permission-rule deltas
//! on `ToolResult::permission_updates`. The executor calls into a
//! [`PermissionRuleHandle`] at the same point it applies an
//! `AppStatePatch`, so subsequent tool calls (and subsequent turns)
//! see the new rules without breaking the prompt-cache prefix.
//!
//! The streaming executor applies permission rule updates to the tool context
//! post-execute. Rust uses a typed callback handle instead of closure-wrapping
//! to keep the data flow trivially `Send + Sync`.
//!
//! # Callback pattern
//!
//! This is the same trait-object decoupling as `AgentHandle`,
//! `HookHandle`, `MailboxHandle`, etc. `coco-tool-runtime` defines the
//! trait; concrete implementations live in higher layers (the CLI's
//! `SessionRuntime` is the canonical wiring) and are injected at
//! executor build time via `with_permission_rule_handle`.
//!
//! # Persistence
//!
//! Rules with destination `Command` / `Session` / `CliArg` are in-memory
//! only — they live in the engine config for the running session and
//! disappear on session end. Disk persistence (settings.json) is the
//! responsibility of separate paths (TUI "Always Allow" with a
//! settings-scoped destination, `/permissions` slash command). This
//! handle never writes to disk.

use async_trait::async_trait;
use std::sync::Arc;

use coco_types::PermissionUpdate;

/// Callback trait for applying permission-rule deltas returned by a
/// tool execution.
///
/// Implementations must be cheap to call — the executor holds the
/// handle by `Arc` and dispatches once per batch (concurrent) or per
/// tool (serial unsafe).
#[async_trait]
pub trait PermissionRuleHandle: Send + Sync {
    /// Fold `updates` into the live session config. `updates` is the
    /// flattened set from the current execution slice (one serial tool,
    /// or one concurrent batch).
    ///
    /// Empty `updates` is a valid no-op — callers do not pre-filter.
    async fn apply_updates(&self, updates: Vec<PermissionUpdate>);
}

/// Shared handle type for the executor / `ToolUseContext`.
pub type PermissionRuleHandleRef = Arc<dyn PermissionRuleHandle>;

/// No-op handle for tests, subagent contexts without runtime config
/// state, and standalone executor uses. Drops updates on the floor with
/// a `tracing::debug!` so a regression where rules never flow through
/// leaves a trail.
#[derive(Debug, Clone, Default)]
pub struct NoOpPermissionRuleHandle;

#[async_trait]
impl PermissionRuleHandle for NoOpPermissionRuleHandle {
    async fn apply_updates(&self, updates: Vec<PermissionUpdate>) {
        if !updates.is_empty() {
            tracing::debug!(
                update_count = updates.len(),
                "NoOpPermissionRuleHandle dropping permission updates"
            );
        }
    }
}
