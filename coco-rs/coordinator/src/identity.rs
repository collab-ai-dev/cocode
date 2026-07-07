//! Teammate identity resolution — 3-tier priority system.
//!
//! Resolution priority:
//! 1. Thread-local context (in-process teammates via `tokio::task_local!`)
//! 2. Dynamic team context (set at runtime for tmux teammates)
//! 3. Environment variables (legacy/fallback)

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use coco_config::EnvKey;
use coco_config::env;
use coco_types::SessionId;
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
    pub parent_session_id: SessionId,
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
    pub parent_session_id: Option<SessionId>,
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

// ── Inherited-env Identity (tier-3, one-shot) ──

/// Cached snapshot of the inherited `COCO_*` identity env vars.
///
/// CC reads `CLAUDE_INTERNAL_ASSISTANT_TEAM_NAME` once at module load and
/// `delete`s it from `process.env`, caching the value at module scope so a
/// teammate's own children (grandchildren) never inherit it. coco-rs mirrors
/// that here: [`consume_inherited_env_identity`] reads all five identity vars
/// into this `OnceLock` **before** removing them, and the tier-3 getters
/// prefer the cache when populated.
#[derive(Debug, Default)]
struct InheritedEnvIdentity {
    agent_id: Option<String>,
    agent_name: Option<String>,
    team_name: Option<String>,
    color: Option<String>,
    plan_mode_required: bool,
}

/// One-shot snapshot of the inherited identity env vars (None until
/// [`consume_inherited_env_identity`] runs).
static INHERITED_ENV: OnceLock<InheritedEnvIdentity> = OnceLock::new();

/// Identity env keys consumed (cached then removed) on teammate startup.
const CONSUMED_IDENTITY_ENV_KEYS: [EnvKey; 5] = [
    AGENT_ID_ENV_VAR,
    AGENT_NAME_ENV_VAR,
    TEAM_NAME_ENV_VAR,
    TEAMMATE_COLOR_ENV_VAR,
    PLAN_MODE_REQUIRED_ENV_VAR,
];

/// Consume the inherited `COCO_*` identity env vars exactly once: cache their
/// values at module scope, then `remove_var` each so a teammate's own children
/// (grandchildren) don't inherit a stale identity. Idempotent — the `OnceLock`
/// guarantees the read+remove runs at most once per process.
///
/// Call this at teammate process startup (the single teammate-detection site).
/// The leader path may also call it harmlessly: with no identity env set, the
/// cache stores `None`s and removes nothing.
///
/// MUST cache before removing (the `OnceLock::get_or_init` body reads env into
/// the snapshot, and only after the snapshot is built do we drop the env vars).
pub fn consume_inherited_env_identity() {
    INHERITED_ENV.get_or_init(|| {
        // Cache first.
        let snapshot = InheritedEnvIdentity {
            agent_id: env::env_opt(AGENT_ID_ENV_VAR),
            agent_name: env::env_opt(AGENT_NAME_ENV_VAR),
            team_name: env::env_opt(TEAM_NAME_ENV_VAR),
            color: env::env_opt(TEAMMATE_COLOR_ENV_VAR),
            plan_mode_required: env::is_env_truthy(PLAN_MODE_REQUIRED_ENV_VAR),
        };
        // Then remove, so grandchildren don't inherit the identity. CC scopes
        // identity to the immediate child; this matches that.
        //
        // SAFETY: `std::env::remove_var` is not thread-safe. This runs once at
        // teammate process startup (the teammate-detection site, before the
        // engine/TUI worker threads that read env are spawned) and is guarded
        // by the `OnceLock` so concurrent callers observe a single mutation.
        unsafe {
            for key in CONSUMED_IDENTITY_ENV_KEYS {
                std::env::remove_var(key.as_str());
            }
        }
        snapshot
    });
}

/// Tier-3 string getter: prefer the consumed one-shot cache, else live env.
fn inherited_env_string(
    key: EnvKey,
    pick: fn(&InheritedEnvIdentity) -> Option<&String>,
) -> Option<String> {
    if let Some(cached) = INHERITED_ENV.get() {
        return pick(cached).cloned();
    }
    env::env_opt(key)
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
    // Tier 3: inherited env (one-shot cache → live env fallback).
    inherited_env_string(AGENT_ID_ENV_VAR, |c| c.agent_id.as_ref())
}

/// Get the current agent display name (3-tier priority).
pub fn get_agent_name() -> Option<String> {
    if let Some(ctx) = get_teammate_context() {
        return Some(ctx.agent_name);
    }
    if let Some(ctx) = get_dynamic_team_context() {
        return Some(ctx.agent_name);
    }
    inherited_env_string(AGENT_NAME_ENV_VAR, |c| c.agent_name.as_ref())
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
    inherited_env_string(TEAM_NAME_ENV_VAR, |c| c.team_name.as_ref())
}

/// Get the parent session ID as a legacy string boundary.
pub fn get_parent_session_id() -> Option<String> {
    checked_parent_session_id()
        .ok()
        .flatten()
        .map(|session_id| session_id.to_string())
}

/// Get the parent session ID with path-component validation.
pub fn checked_parent_session_id() -> Result<Option<SessionId>, String> {
    if let Some(ctx) = get_teammate_context() {
        return Ok(Some(ctx.parent_session_id));
    }
    if let Some(ctx) = get_dynamic_team_context() {
        return Ok(ctx.parent_session_id);
    }
    let Some(session_id) = env::env_opt(PARENT_SESSION_ID_ENV_VAR) else {
        return Ok(None);
    };
    SessionId::try_new(session_id)
        .map(Some)
        .map_err(|e| format!("{} is invalid: {e}", PARENT_SESSION_ID_ENV_VAR.as_str()))
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
    inherited_env_string(TEAMMATE_COLOR_ENV_VAR, |c| c.color.as_ref())
}

/// Check if plan mode is required.
pub fn is_plan_mode_required() -> bool {
    if let Some(ctx) = get_teammate_context() {
        return ctx.plan_mode_required;
    }
    if let Some(ctx) = get_dynamic_team_context() {
        return ctx.plan_mode_required;
    }
    if let Some(cached) = INHERITED_ENV.get() {
        return cached.plan_mode_required;
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
    parent_session_id: SessionId,
) -> TeammateContextData {
    TeammateContextData {
        agent_id: format!("{agent_name}@{team_name}"),
        agent_name: agent_name.to_string(),
        team_name: team_name.to_string(),
        color,
        plan_mode_required,
        parent_session_id,
        // Spawn-time context carries no runner cancel flag — the runner
        // loop wires its own when it scopes the per-turn context.
        self_stop_signal: None,
    }
}

#[cfg(test)]
#[path = "identity.test.rs"]
mod tests;
