//! Teammate identity resolution — 3-tier priority system.
//!
//! Resolution priority:
//! 1. Thread-local context (in-process teammates via `tokio::task_local!`)
//! 2. Dynamic team context (set at runtime for tmux teammates)
//! 3. Environment variables (legacy/fallback)

use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use coco_config::env;
use coco_types::TaskStateBase;

use crate::constants::AGENT_ID_ENV_VAR;
use crate::constants::AGENT_NAME_ENV_VAR;
use crate::constants::PARENT_SESSION_ID_ENV_VAR;
use crate::constants::PLAN_MODE_REQUIRED_ENV_VAR;
use crate::constants::TEAM_NAME_ENV_VAR;
use crate::constants::TEAMMATE_COLOR_ENV_VAR;

// ── Thread-local Context (tier 1) ──

tokio::task_local! {
    /// Thread-local teammate context for in-process teammates.
    /// Set via `run_with_teammate_context()`.
    static TEAMMATE_CONTEXT: TeammateContextData;
}

/// Runtime context for in-process teammates (stored in task-local).
#[derive(Debug, Clone)]
pub struct TeammateContextData {
    pub agent_id: String,
    pub agent_name: String,
    pub team_name: String,
    pub color: Option<String>,
    pub plan_mode_required: bool,
    pub parent_session_id: String,
    /// In-process teammate's own stop flag (the runner-loop's
    /// `config.cancelled`). When the model approves its own shutdown,
    /// [`signal_self_stop`] flips this so the runner loop breaks on its
    /// next `config.cancelled` check — the in-process analog of TS
    /// `handleShutdownApproval` aborting the teammate's `abortController`.
    /// `None` for non-runner contexts (spawn-time / tmux).
    pub self_stop_signal: Option<Arc<AtomicBool>>,
}

/// Run a future with teammate context set in task-local storage.
pub async fn run_with_teammate_context<F, T>(context: TeammateContextData, f: F) -> T
where
    F: std::future::Future<Output = T>,
{
    TEAMMATE_CONTEXT.scope(context, f).await
}

/// Get the current teammate context from task-local (if any).
pub fn get_teammate_context() -> Option<TeammateContextData> {
    TEAMMATE_CONTEXT.try_with(std::clone::Clone::clone).ok()
}

/// Check if running as an in-process teammate.
pub fn is_in_process_teammate() -> bool {
    TEAMMATE_CONTEXT.try_with(|_| ()).is_ok()
}

/// Signal the current in-process teammate to stop after this turn by
/// flipping its task-local `self_stop_signal`. Returns `true` when a
/// signal was present (an in-process teammate with a wired flag).
///
/// Called from `SendMessageTool` → `respond_to_shutdown` on the APPROVE
/// path: the tool runs inline within the teammate's task-local scope, so
/// it can flip the runner-loop's own `config.cancelled` Arc and let the
/// loop exit on its next cancellation check.
pub fn signal_self_stop() -> bool {
    TEAMMATE_CONTEXT
        .try_with(|ctx| {
            if let Some(sig) = &ctx.self_stop_signal {
                sig.store(true, Ordering::Relaxed);
                true
            } else {
                false
            }
        })
        .unwrap_or(false)
}

// ── Dynamic Context (tier 2) ──

/// Module-scoped dynamic team context (for tmux teammates).
static DYNAMIC_CONTEXT: RwLock<Option<DynamicTeamContext>> = RwLock::new(None);

/// Dynamic team context set at runtime (not via task-local).
#[derive(Debug, Clone)]
pub struct DynamicTeamContext {
    pub agent_id: String,
    pub agent_name: String,
    pub team_name: String,
    pub color: Option<String>,
    pub plan_mode_required: bool,
    pub parent_session_id: Option<String>,
}

/// Set the dynamic team context.
pub fn set_dynamic_team_context(ctx: DynamicTeamContext) {
    if let Ok(mut guard) = DYNAMIC_CONTEXT.write() {
        *guard = Some(ctx);
    }
}

/// Clear the dynamic team context.
pub fn clear_dynamic_team_context() {
    if let Ok(mut guard) = DYNAMIC_CONTEXT.write() {
        *guard = None;
    }
}

/// Get the dynamic team context.
pub fn get_dynamic_team_context() -> Option<DynamicTeamContext> {
    DYNAMIC_CONTEXT.read().ok().and_then(|g| g.clone())
}

// ── Identity Resolution (3-tier) ──

/// Get the current agent ID (3-tier priority).
pub fn get_agent_id() -> Option<String> {
    // Tier 1: task-local
    if let Some(ctx) = get_teammate_context() {
        return Some(ctx.agent_id);
    }
    // Tier 2: dynamic context
    if let Some(ctx) = get_dynamic_team_context() {
        return Some(ctx.agent_id);
    }
    // Tier 3: env var (cross-process fallback).
    env::env_opt(AGENT_ID_ENV_VAR)
}

/// Get the current agent display name (3-tier priority).
pub fn get_agent_name() -> Option<String> {
    if let Some(ctx) = get_teammate_context() {
        return Some(ctx.agent_name);
    }
    if let Some(ctx) = get_dynamic_team_context() {
        return Some(ctx.agent_name);
    }
    env::env_opt(AGENT_NAME_ENV_VAR)
}

/// Get the current team name (3-tier priority: task-local → dynamic → env).
///
/// The optional `teamContext` arg from the original design is omitted:
/// no production caller ever supplied it (the live authority is the coordinator
/// roster, not an `AppState.teamContext`).
pub fn get_team_name() -> Option<String> {
    if let Some(ctx) = get_teammate_context() {
        return Some(ctx.team_name);
    }
    if let Some(ctx) = get_dynamic_team_context() {
        return Some(ctx.team_name);
    }
    env::env_opt(TEAM_NAME_ENV_VAR)
}

/// Get the parent session ID.
pub fn get_parent_session_id() -> Option<String> {
    if let Some(ctx) = get_teammate_context() {
        return Some(ctx.parent_session_id);
    }
    if let Some(ctx) = get_dynamic_team_context() {
        return ctx.parent_session_id;
    }
    env::env_opt(PARENT_SESSION_ID_ENV_VAR)
}

/// Check if currently running as a teammate (not leader).
pub fn is_teammate() -> bool {
    get_agent_id().is_some()
}

/// Get the teammate's assigned UI color.
pub fn get_teammate_color() -> Option<String> {
    if let Some(ctx) = get_teammate_context() {
        return ctx.color;
    }
    if let Some(ctx) = get_dynamic_team_context() {
        return ctx.color;
    }
    env::env_opt(TEAMMATE_COLOR_ENV_VAR)
}

/// Check if plan mode is required.
pub fn is_plan_mode_required() -> bool {
    if let Some(ctx) = get_teammate_context() {
        return ctx.plan_mode_required;
    }
    if let Some(ctx) = get_dynamic_team_context() {
        return ctx.plan_mode_required;
    }
    env::is_env_truthy(PLAN_MODE_REQUIRED_ENV_VAR)
}

/// Check if there are any active in-process teammates.
pub fn has_active_in_process_teammates(tasks: &[TaskStateBase]) -> bool {
    tasks.iter().any(|t| {
        t.teammate_extras()
            .is_some_and(|e| !e.is_idle && !e.shutdown_requested)
    })
}

/// Check if there are any working (non-idle) in-process teammates.
pub fn has_working_in_process_teammates(tasks: &[TaskStateBase]) -> bool {
    tasks
        .iter()
        .any(|t| t.teammate_extras().is_some_and(|e| !e.is_idle))
}

/// Wait for all in-process teammates to become idle. Polls the
/// supplied snapshot fn every 500ms until idle.
pub async fn wait_for_teammates_to_become_idle<F, Fut>(snapshot: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Vec<TaskStateBase>>,
{
    loop {
        let tasks = snapshot().await;
        if !has_working_in_process_teammates(&tasks) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Resolve the current process's teammate identity from the 3-tier
/// context (task-local → dynamic → env). Returns `None` when this is not
/// a teammate (no agent id resolves).
///
/// Used by the cross-process [`MailboxPermissionBridge`] install so a
/// pane teammate forwards deny-path permission prompts to the leader.
///
/// [`MailboxPermissionBridge`]: crate::runner_loop_mailbox_permission::MailboxPermissionBridge
pub fn resolve_teammate_identity() -> Option<crate::types::TeammateIdentity> {
    use std::str::FromStr;
    let agent_id = get_agent_id()?;
    let team_name = get_team_name()?;
    let agent_name = get_agent_name().unwrap_or_else(|| agent_id.clone());
    let color = get_teammate_color().and_then(|c| coco_types::AgentColorName::from_str(&c).ok());
    Some(crate::types::TeammateIdentity {
        agent_id,
        agent_name,
        team_name,
        color,
        plan_mode_required: is_plan_mode_required(),
    })
}

/// Create a `TeammateContextData` for spawning an in-process agent.
pub fn create_teammate_context(
    agent_name: &str,
    team_name: &str,
    color: Option<String>,
    plan_mode_required: bool,
    parent_session_id: &str,
) -> TeammateContextData {
    TeammateContextData {
        agent_id: format!("{agent_name}@{team_name}"),
        agent_name: agent_name.to_string(),
        team_name: team_name.to_string(),
        color,
        plan_mode_required,
        parent_session_id: parent_session_id.to_string(),
        // Spawn-time context carries no runner cancel flag — the runner
        // loop wires its own when it scopes the per-turn context.
        self_stop_signal: None,
    }
}

#[cfg(test)]
#[path = "identity.test.rs"]
mod tests;
